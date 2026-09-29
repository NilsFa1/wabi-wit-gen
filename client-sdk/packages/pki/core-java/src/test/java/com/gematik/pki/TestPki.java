// Builds a small test PKI with BouncyCastle (a gemLibPki dependency): a CA
// listed in a TSL, and end-entity certificates for TUC_PKI_018 tests. Real
// gematik test data is not needed (and not redistributable here).
package com.gematik.pki;

import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.spec.ECGenParameterSpec;
import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.Base64;
import java.util.Date;
import org.bouncycastle.asn1.ASN1ObjectIdentifier;
import org.bouncycastle.asn1.DERSequence;
import org.bouncycastle.asn1.isismtt.ISISMTTObjectIdentifiers;
import org.bouncycastle.asn1.isismtt.x509.AdmissionSyntax;
import org.bouncycastle.asn1.isismtt.x509.Admissions;
import org.bouncycastle.asn1.isismtt.x509.ProfessionInfo;
import org.bouncycastle.asn1.x500.DirectoryString;
import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.asn1.x509.BasicConstraints;
import org.bouncycastle.asn1.x509.CertificatePolicies;
import org.bouncycastle.asn1.x509.ExtendedKeyUsage;
import org.bouncycastle.asn1.x509.Extension;
import org.bouncycastle.asn1.x509.GeneralName;
import org.bouncycastle.asn1.x509.KeyPurposeId;
import org.bouncycastle.asn1.x509.KeyUsage;
import org.bouncycastle.asn1.x509.PolicyInformation;
import org.bouncycastle.cert.X509CertificateHolder;
import org.bouncycastle.cert.jcajce.JcaX509ExtensionUtils;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;

public final class TestPki {
    /** gemSpec_OID oid_smc_b_aut (certificate type of C.HCI.AUT) */
    static final String OID_SMC_B_AUT = "1.2.276.0.76.4.77";
    /** gemSpec_OID oid_praxis_arzt (profession OID) */
    static final String OID_PRAXIS_ARZT = "1.2.276.0.76.4.50";

    final byte[] tsl;
    final byte[] valid, expired, wrongKeyUsage, unknownCa;

    private static long serial = 1000;

    private static KeyPair ecKey() throws Exception {
        var gen = KeyPairGenerator.getInstance("EC");
        gen.initialize(new ECGenParameterSpec("secp256r1"));
        return gen.generateKeyPair();
    }

    private static X509CertificateHolder ca(String cn, KeyPair key) throws Exception {
        var name = new X500Name("CN=" + cn + ",O=gematik PoC Test,C=DE");
        var b = new JcaX509v3CertificateBuilder(name, BigInteger.valueOf(serial++),
                Date.from(Instant.now().minus(1, ChronoUnit.DAYS)), Date.from(Instant.now().plus(7300, ChronoUnit.DAYS)),
                name, key.getPublic());
        var ext = new JcaX509ExtensionUtils();
        b.addExtension(Extension.basicConstraints, true, new BasicConstraints(0));
        b.addExtension(Extension.keyUsage, true, new KeyUsage(KeyUsage.keyCertSign | KeyUsage.cRLSign));
        b.addExtension(Extension.subjectKeyIdentifier, false, ext.createSubjectKeyIdentifier(key.getPublic()));
        return b.build(new JcaContentSignerBuilder("SHA256withECDSA").build(key.getPrivate()));
    }

    private static byte[] ee(X509CertificateHolder ca, KeyPair caKey, Instant from, Instant to, int keyUsage) throws Exception {
        var key = ecKey();
        var subject = new X500Name("CN=Praxis Dr. Test,O=gematik PoC Test,C=DE");
        var b = new JcaX509v3CertificateBuilder(ca.getSubject(), BigInteger.valueOf(serial++),
                Date.from(from), Date.from(to), subject, key.getPublic());
        var ext = new JcaX509ExtensionUtils();
        b.addExtension(Extension.basicConstraints, true, new BasicConstraints(false)); // gemSpec_PKI: critical, as keyUsage
        b.addExtension(Extension.keyUsage, true, new KeyUsage(keyUsage));
        b.addExtension(Extension.extendedKeyUsage, false, new ExtendedKeyUsage(KeyPurposeId.id_kp_clientAuth));
        b.addExtension(Extension.certificatePolicies, false,
                new CertificatePolicies(new PolicyInformation(new ASN1ObjectIdentifier(OID_SMC_B_AUT))));
        b.addExtension(Extension.authorityKeyIdentifier, false, ext.createAuthorityKeyIdentifier(caKey.getPublic()));
        b.addExtension(Extension.subjectKeyIdentifier, false, ext.createSubjectKeyIdentifier(key.getPublic()));
        var profession = new ProfessionInfo(null,
                new DirectoryString[] {new DirectoryString("Betriebsstätte Arzt")},
                new ASN1ObjectIdentifier[] {new ASN1ObjectIdentifier(OID_PRAXIS_ARZT)},
                "1-2-ARZT-Test-01", null);
        var admission = new AdmissionSyntax(
                new GeneralName(new X500Name("CN=Kassenärztliche Vereinigung Test,C=DE")),
                new DERSequence(new Admissions(null, null, new ProfessionInfo[] {profession})));
        b.addExtension(ISISMTTObjectIdentifiers.id_isismtt_at_admission, false, admission);
        return b.build(new JcaContentSignerBuilder("SHA256withECDSA").build(caKey.getPrivate())).getEncoded();
    }

    /** A minimal ETSI TS 119 612 TSL listing `ca` as a gematik CA allowed to issue SMC-B AUT certificates. */
    private static byte[] tsl(X509CertificateHolder ca) throws Exception {
        var cert = Base64.getEncoder().encodeToString(ca.getEncoded());
        var xml = """
                <?xml version="1.0" encoding="UTF-8"?>
                <TrustServiceStatusList xmlns="http://uri.etsi.org/02231/v2#" Id="TSL-PoC" TSLTag="http://uri.etsi.org/02231/TSLTag">
                  <SchemeInformation>
                    <TSLVersionIdentifier>3</TSLVersionIdentifier>
                    <TSLSequenceNumber>1</TSLSequenceNumber>
                    <TSLType>http://uri.etsi.org/TrstSvc/TrustedList/TSLType/EUgeneric</TSLType>
                    <SchemeOperatorName><Name xml:lang="DE">gematik PoC Test</Name></SchemeOperatorName>
                  </SchemeInformation>
                  <TrustServiceProviderList>
                    <TrustServiceProvider>
                      <TSPInformation><TSPName><Name xml:lang="DE">PoC TSP</Name></TSPName></TSPInformation>
                      <TSPServices>
                        <TSPService>
                          <ServiceInformation>
                            <ServiceTypeIdentifier>http://uri.etsi.org/TrstSvc/Svctype/CA/PKC</ServiceTypeIdentifier>
                            <ServiceName><Name xml:lang="DE">PoC SMC-B CA</Name></ServiceName>
                            <ServiceDigitalIdentity><DigitalId><X509Certificate>%s</X509Certificate></DigitalId></ServiceDigitalIdentity>
                            <ServiceStatus>http://uri.etsi.org/TrstSvc/Svcstatus/inaccord</ServiceStatus>
                            <StatusStartingTime>2020-01-01T00:00:00Z</StatusStartingTime>
                            <ServiceSupplyPoints><ServiceSupplyPoint>http://ocsp.invalid/ocsp</ServiceSupplyPoint></ServiceSupplyPoints>
                            <ServiceInformationExtensions>
                              <Extension Critical="false"><ExtensionOID>%s</ExtensionOID></Extension>
                            </ServiceInformationExtensions>
                          </ServiceInformation>
                        </TSPService>
                      </TSPServices>
                    </TrustServiceProvider>
                  </TrustServiceProviderList>
                </TrustServiceStatusList>
                """.formatted(cert, OID_SMC_B_AUT);
        return xml.getBytes(StandardCharsets.UTF_8);
    }

    public TestPki() throws Exception {
        var caKey = ecKey();
        var ca = ca("PoC SMC-B CA", caKey);
        var now = Instant.now();
        var longAgo = now.minus(1, ChronoUnit.DAYS);
        var future = now.plus(7000, ChronoUnit.DAYS);
        tsl = tsl(ca);
        valid = ee(ca, caKey, longAgo, future, KeyUsage.digitalSignature);
        expired = ee(ca, caKey, Instant.parse("2020-01-01T00:00:00Z"), Instant.parse("2021-01-01T00:00:00Z"), KeyUsage.digitalSignature);
        wrongKeyUsage = ee(ca, caKey, longAgo, future, KeyUsage.keyAgreement); // C.HCI.AUT needs digitalSignature
        var otherKey = ecKey();
        unknownCa = ee(ca("Unknown CA", otherKey), otherKey, longAgo, future, KeyUsage.digitalSignature);
    }

    /**
     * Writes the fixtures used by the SDK tests (C#, Java, Dart, Swift) unless
     * they exist already (they are committed; certificates are valid ~19 years).
     */
    public void write(Path dir) throws Exception {
        if (Files.exists(dir.resolve("tsl.xml"))) return;
        Files.createDirectories(dir);
        Files.write(dir.resolve("tsl.xml"), tsl);
        Files.write(dir.resolve("valid.der"), valid);
        Files.write(dir.resolve("expired.der"), expired);
        Files.write(dir.resolve("wrong-key-usage.der"), wrongKeyUsage);
        Files.write(dir.resolve("unknown-ca.der"), unknownCa);
    }

    public static void main(String[] args) throws Exception {
        new TestPki().write(Path.of(args.length > 0 ? args[0] : "fixtures"));
    }
}
