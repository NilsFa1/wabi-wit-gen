// The Dart/Flutter app side of gematik:pki: certificate verification with
// gemLibPki (Java) running as a GraalVM native library — no JVM needed.
// The core under test is chosen with ACME_NATIVE_LIB, fixtures with PKI_FIXTURES.
import 'dart:io';
import 'dart:typed_data';

import 'package:gematik_pki/gematik_pki.dart';
import 'package:test/test.dart';

Uint8List load(String name) =>
    File('${Platform.environment['PKI_FIXTURES'] ?? '../../tests/fixtures'}/$name').readAsBytesSync();

Matcher failsWith(String code) => throwsA(isA<PkiErrorVerificationFailed>().having((e) => e.value.code, 'code', code));

void main() {
  late CertificateVerifier verifier;
  const aut = [CertificateProfile.cHciAutEcc];

  setUpAll(() {
    print('core under test: ${NativeBackend.shared.description}');
    verifier = CertificateVerifier('PoC-Test', load('tsl.xml'));
  });
  tearDownAll(() => verifier.close());

  test('valid SMC-B certificate', () async {
    final a = await verifier.verify(load('valid.der'), aut, false);
    expect(a.profile, CertificateProfile.cHciAutEcc);
    expect(a.registrationNumber, '1-2-ARZT-Test-01');
    expect(a.professionItems, ['Betriebsstätte Arzt']);
    expect(a.professionOids, ['1.2.276.0.76.4.50']);
  });

  test('first matching profile wins', () async {
    final a = await verifier.verify(load('valid.der'), [CertificateProfile.cHpAutEcc, CertificateProfile.cHciAutEcc], false);
    expect(a.profile, CertificateProfile.cHciAutEcc);
  });

  test('expired certificate', () {
    expect(verifier.verify(load('expired.der'), aut, false), failsWith('SE_1021_CERTIFICATE_NOT_VALID_TIME'));
  });

  test('wrong key usage', () {
    expect(verifier.verify(load('wrong-key-usage.der'), aut, false), failsWith('SE_1016_WRONG_KEYUSAGE'));
  });

  test('issuer not in the TSL', () {
    expect(verifier.verify(load('unknown-ca.der'), aut, false), failsWith('TE_1027_CA_CERT_MISSING'));
  });

  test('invalid input', () {
    expect(verifier.verify(Uint8List.fromList([1, 2, 3]), aut, false), throwsA(isA<PkiErrorInvalidInput>()));
  });

  test('concurrent verifications', () async {
    final all = await Future.wait([for (var i = 0; i < 16; i++) verifier.verify(load('valid.der'), aut, false)]);
    expect(all.every((a) => a.registrationNumber == '1-2-ARZT-Test-01'), isTrue);
  });
}
