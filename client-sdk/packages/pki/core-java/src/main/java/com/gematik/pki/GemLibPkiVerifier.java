// The small wrapper around gematik ref-GemLibPki: TUC_PKI_018 certificate
// verification (TucPki018Verifier) against the trust anchors of a TSL.
package com.gematik.pki;

import de.gematik.pki.gemlibpki.exception.GemPkiException;
import de.gematik.pki.gemlibpki.exception.GemPkiRuntimeException;
import de.gematik.pki.gemlibpki.certificate.TucPki018Verifier;
import de.gematik.pki.gemlibpki.tsl.TslConverter;
import de.gematik.pki.gemlibpki.tsl.TslInformationProvider;
import de.gematik.pki.gemlibpki.tsl.TspService;
import de.gematik.pki.gemlibpki.ocsp.OcspRespCache;
import java.io.ByteArrayInputStream;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.Duration;
import java.time.ZoneOffset;
import java.time.ZonedDateTime;
import java.time.format.DateTimeFormatter;
import java.util.HexFormat;
import java.util.Optional;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;

final class GemLibPkiVerifier implements CertificateVerifierResource {
    /** OCSP responses are reused for this long (gemLibPki's OcspRespCache, keyed by serial number). */
    static final int OCSP_CACHE_SECONDS = 300;

    /** The TSL in use: immutable, swapped atomically by setTsl / refreshTsl. */
    record Snapshot(byte[] raw, String sha256, List<TspService> services, TslInfo info, ZonedDateTime nextUpdate) {}

    private final String productType;
    private final byte[] initialTsl; // parsed on first use (the constructor cannot fail)
    private volatile Snapshot snapshot;
    private final OcspRespCache ocspCache = new OcspRespCache(OCSP_CACHE_SECONDS);
    private final HttpClient http = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(10)).build();

    GemLibPkiVerifier(String productType, byte[] tsl) {
        this.productType = productType;
        this.initialTsl = tsl.clone();
    }

    private static PkiException invalid(String message) {
        return new PkiException(new PkiError.InvalidInput(message));
    }

    private static String sha256Hex(byte[] b) {
        try {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(b));
        } catch (java.security.NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    private static String rfc3339(javax.xml.datatype.XMLGregorianCalendar c) {
        return c == null ? "" : c.toGregorianCalendar().toZonedDateTime().withZoneSameInstant(ZoneOffset.UTC)
                .truncatedTo(java.time.temporal.ChronoUnit.SECONDS).format(DateTimeFormatter.ISO_OFFSET_DATE_TIME)
                .replace("+00:00", "Z");
    }

    static Snapshot parse(byte[] raw) {
        final eu.europa.esig.trustedlist.jaxb.tsl.TrustStatusListType parsed;
        try {
            parsed = TslConverter.bytesToTsl(raw).orElseThrow(() -> invalid("TSL could not be parsed"));
        } catch (GemPkiRuntimeException e) {
            throw invalid("TSL could not be parsed: " + e.getMessage());
        }
        if (parsed.getTrustServiceProviderList() == null) throw invalid("TSL lists no trust service providers");
        var si = parsed.getSchemeInformation();
        var next = si == null || si.getNextUpdate() == null ? null : si.getNextUpdate().getDateTime();
        var hash = sha256Hex(raw);
        var info = new TslInfo(
                Objects.toString(parsed.getId(), ""),
                si == null || si.getTSLSequenceNumber() == null ? 0 : si.getTSLSequenceNumber().longValue(),
                si == null ? "" : rfc3339(si.getListIssueDateTime()),
                rfc3339(next),
                hash,
                false);
        return new Snapshot(raw.clone(), hash, new TslInformationProvider(parsed).getTspServices(), info,
                next == null ? null : next.toGregorianCalendar().toZonedDateTime());
    }

    private Snapshot current() {
        var s = snapshot;
        if (s == null) {
            synchronized (this) {
                if (snapshot == null) snapshot = parse(initialTsl);
                s = snapshot;
            }
        }
        return s;
    }

    private static TslInfo infoAt(Snapshot s) {
        var expired = s.nextUpdate() != null && ZonedDateTime.now(ZoneOffset.UTC).isAfter(s.nextUpdate());
        var i = s.info();
        return new TslInfo(i.id(), i.sequenceNumber(), i.issueDate(), i.nextUpdate(), i.sha256(), expired);
    }

    @Override
    public TslInfo tslInfo() {
        return infoAt(current());
    }

    @Override
    public TslInfo setTsl(byte[] tsl) {
        var s = parse(tsl); // throws on a TSL that does not parse: the current one stays
        synchronized (this) {
            snapshot = s;
        }
        return infoAt(s);
    }

    @Override
    public CompletionStage<Optional<byte[]>> refreshTsl(String url) {
        if (!url.toLowerCase(java.util.Locale.ROOT).endsWith(".xml")) {
            return CompletableFuture.failedFuture(downloadFailed("TSL URL must end in .xml: " + url));
        }
        String current;
        try {
            current = current().sha256();
        } catch (PkiException e) {
            current = "";
        }
        final var known = current;
        var sha2Url = url.substring(0, url.length() - 4) + ".sha2";
        return get(sha2Url).thenCompose(hashBytes -> {
            var announced = new String(hashBytes, StandardCharsets.US_ASCII).trim().toLowerCase(java.util.Locale.ROOT);
            if (announced.equals(known)) return CompletableFuture.completedFuture(Optional.<byte[]>empty());
            return get(url).thenApply(raw -> {
                var got = sha256Hex(raw);
                if (!got.equals(announced)) {
                    throw downloadFailed("downloaded TSL has SHA-256 " + got + ", " + sha2Url + " announces " + announced);
                }
                setTsl(raw);
                return Optional.of(raw);
            });
        });
    }

    private static PkiException downloadFailed(String message) {
        return new PkiException(new PkiError.DownloadFailed(message));
    }

    private CompletableFuture<byte[]> get(String url) {
        HttpRequest req;
        try {
            req = HttpRequest.newBuilder(URI.create(url)).timeout(Duration.ofSeconds(30)).GET().build();
        } catch (IllegalArgumentException e) {
            return CompletableFuture.failedFuture(downloadFailed("invalid URL " + url + ": " + e.getMessage()));
        }
        return http.sendAsync(req, HttpResponse.BodyHandlers.ofByteArray())
                .handle((resp, err) -> {
                    if (err != null) throw downloadFailed("GET " + url + ": " + err);
                    if (resp.statusCode() != 200) throw downloadFailed("GET " + url + ": HTTP " + resp.statusCode());
                    return resp.body();
                });
    }

    @Override
    public CacheStats cacheStats() {
        // gemLibPki's cache exposes its size only
        return new CacheStats(0, 0, ocspCache.getSize());
    }

    /** gemLibPki's Admission accessors throw if the certificate has no admission extension. */
    private static String orEmpty(java.util.function.Supplier<String> get) {
        try {
            return Objects.toString(get.get(), "");
        } catch (RuntimeException e) {
            return "";
        }
    }

    private static java.util.Set<String> orEmptySet(java.util.function.Supplier<java.util.Set<String>> get) {
        try {
            return new java.util.TreeSet<>(get.get());
        } catch (RuntimeException e) {
            return java.util.Set.of();
        }
    }

    /** WIT enum -> gemLibPki enum (whose names are not uniform). */
    static de.gematik.pki.gemlibpki.certificate.CertificateProfile toGemLib(CertificateProfile p) {
        return switch (p) {
            case C_CH_AUT_RSA -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_CH_AUT_RSA;
            case C_CH_AUT_ECC -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_CH_AUT_ECC;
            case C_HP_AUT_RSA -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_HP_AUT_RSA;
            case C_HP_AUT_ECC -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_HP_AUT_ECC;
            case C_HCI_AUT_RSA -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_HCI_AUT_RSA;
            case C_HCI_AUT_ECC -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_HCI_AUT_ECC;
            case C_HCI_ENC_RSA -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_HCI_ENC_RSA;
            case C_HCI_ENC_ECC -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.C_HCI_ENC_ECC;
            case C_HCI_OSIG -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_HCI_OSIG;
            case C_FD_SIG -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_FD_SIG;
            case C_FD_OSIG -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_FD_OSIG;
            case C_TSL_SIG -> de.gematik.pki.gemlibpki.certificate.CertificateProfile.CERT_PROFILE_C_TSL_SIG;
        };
    }

    @Override
    public CompletionStage<Admission> verify(byte[] certificate, List<CertificateProfile> profiles, boolean withOcsp) {
        try {
            return CompletableFuture.completedFuture(verifyNow(certificate, profiles, withOcsp));
        } catch (PkiException e) {
            return CompletableFuture.failedFuture(e);
        }
    }

    private Admission verifyNow(byte[] certificate, List<CertificateProfile> profiles, boolean withOcsp) {
        if (profiles.isEmpty()) throw invalid("at least one certificate profile is required");
        final X509Certificate cert;
        try {
            cert = (X509Certificate) CertificateFactory.getInstance("X.509")
                    .generateCertificate(new ByteArrayInputStream(certificate));
        } catch (CertificateException | RuntimeException e) {
            throw invalid("certificate is not a DER-encoded X.509 certificate: " + e.getMessage());
        }
        var services = current().services();

        // Like gemLibPki: the first profile that verifies wins; otherwise report
        // the first error (code) and all messages. Checking one profile at a time
        // also tells the caller which profile matched.
        var messages = new ArrayList<String>();
        GemPkiException first = null;
        for (var profile : profiles) {
            try {
                var admission = TucPki018Verifier.builder()
                        .productType(productType)
                        .tspServiceList(services)
                        .certificateProfiles(List.of(toGemLib(profile)))
                        .withOcspCheck(withOcsp)
                        .ocspRespCache(ocspCache)
                        .build()
                        .performTucPki18Checks(cert);
                return new Admission(
                        profile,
                        orEmpty(admission::getAdmissionAuthority),
                        List.copyOf(orEmptySet(admission::getProfessionItems)),
                        List.copyOf(orEmptySet(admission::getProfessionOids)),
                        orEmpty(admission::getRegistrationNumber));
            } catch (GemPkiException e) {
                if (first == null) first = e;
                messages.add(e.getMessage() + " (" + profile + ")");
            } catch (GemPkiRuntimeException | IllegalArgumentException e) {
                throw invalid(e.getMessage());
            }
        }
        throw new PkiException(new PkiError.VerificationFailed(
                new VerificationFailure(first.getError().name(), String.join("; ", messages))));
    }
}
