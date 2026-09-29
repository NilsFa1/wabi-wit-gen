// The small wrapper around gematik ref-GemLibPki: TUC_PKI_018 certificate
// verification (TucPki018Verifier) against the trust anchors of a TSL.
package com.gematik.pki;

import de.gematik.pki.gemlibpki.exception.GemPkiException;
import de.gematik.pki.gemlibpki.exception.GemPkiRuntimeException;
import de.gematik.pki.gemlibpki.certificate.TucPki018Verifier;
import de.gematik.pki.gemlibpki.tsl.TslConverter;
import de.gematik.pki.gemlibpki.tsl.TslInformationProvider;
import de.gematik.pki.gemlibpki.tsl.TspService;
import java.io.ByteArrayInputStream;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;

final class GemLibPkiVerifier implements CertificateVerifierResource {
    private final String productType;
    private final byte[] tsl;
    private volatile List<TspService> tspServices; // parsed on first use

    GemLibPkiVerifier(String productType, byte[] tsl) {
        this.productType = productType;
        this.tsl = tsl.clone();
    }

    private static PkiException invalid(String message) {
        return new PkiException(new PkiError.InvalidInput(message));
    }

    private List<TspService> tspServices() {
        var services = tspServices;
        if (services == null) {
            final eu.europa.esig.trustedlist.jaxb.tsl.TrustStatusListType parsed;
            try {
                parsed = TslConverter.bytesToTsl(tsl).orElseThrow(() -> invalid("TSL could not be parsed"));
            } catch (GemPkiRuntimeException e) {
                throw invalid("TSL could not be parsed: " + e.getMessage());
            }
            if (parsed.getTrustServiceProviderList() == null) throw invalid("TSL lists no trust service providers");
            tspServices = services = new TslInformationProvider(parsed).getTspServices();
        }
        return services;
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
        var services = tspServices();

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
