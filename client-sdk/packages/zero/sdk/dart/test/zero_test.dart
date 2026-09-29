// The app side of host calls in Dart/Flutter: the app owns the key (here a
// pointycastle key standing in for the Secure Enclave / Android Keystore),
// the Go core (libzero) builds the DPoP proof and asks the app to sign.
// The core under test is chosen with ACME_NATIVE_LIB.
import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:gematik_zero/gematik_zero.dart';
import 'package:pointycastle/export.dart';
import 'package:test/test.dart';

final _curve = ECCurve_secp256r1();

SecureRandom _random() {
  final seed = Random.secure();
  return FortunaRandom()..seed(KeyParameter(Uint8List.fromList(List.generate(32, (_) => seed.nextInt(256)))));
}

Uint8List _sha256(List<int> data) => SHA256Digest().process(Uint8List.fromList(data));

String _b64url(List<int> b) => base64Url.encode(b).replaceAll('=', '');

Uint8List _unb64url(String s) => base64Url.decode(s.padRight((s.length + 3) ~/ 4 * 4, '='));

Uint8List _unsigned(BigInt v, [int? width]) {
  var hex = v.toRadixString(16);
  if (hex.length.isOdd) hex = '0$hex';
  final out = [for (var i = 0; i < hex.length; i += 2) int.parse(hex.substring(i, i + 2), radix: 16)];
  if (width != null) return Uint8List.fromList(List.filled(width - out.length, 0) + out);
  return Uint8List.fromList(out);
}

List<int> _derInt(BigInt v) {
  var b = _unsigned(v);
  if (b[0] & 0x80 != 0) b = Uint8List.fromList([0, ...b]);
  return [0x02, b.length, ...b];
}

/// The "keystore" of the app. Platform keystores return SPKI DER public keys
/// and ASN.1 DER signatures; this one does the same.
final class AppKeys implements HostKeys {
  AppKeys() {
    final gen = ECKeyGenerator()..init(ParametersWithRandom(ECKeyGeneratorParameters(_curve), _random()));
    final pair = gen.generateKeyPair();
    private = pair.privateKey as ECPrivateKey;
    public = pair.publicKey as ECPublicKey;
  }

  late final ECPrivateKey private;
  late final ECPublicKey public;
  var signatures = 0;

  Uint8List get x => _unsigned(public.Q!.x!.toBigInteger()!, 32);
  Uint8List get y => _unsigned(public.Q!.y!.toBigInteger()!, 32);

  @override
  Future<Uint8List> publicKey(String keyId) async {
    if (keyId != 'dpop-key') throw ZeroErrorKeyNotFound(keyId);
    // SubjectPublicKeyInfo for id-ecPublicKey / prime256v1 + uncompressed point
    const prefix = '3059301306072a8648ce3d020106082a8648ce3d030107034200';
    final head = [for (var i = 0; i < prefix.length; i += 2) int.parse(prefix.substring(i, i + 2), radix: 16)];
    return Uint8List.fromList([...head, 0x04, ...x, ...y]);
  }

  @override
  Future<Uint8List> sign(String keyId, Uint8List digest) async {
    if (keyId != 'dpop-key') throw ZeroErrorKeyNotFound(keyId);
    await Future<void>.delayed(const Duration(milliseconds: 1)); // e.g. a biometric prompt
    final signer = ECDSASigner(null, HMac(SHA256Digest(), 64)) // RFC 6979, digest given by the core
      ..init(true, PrivateKeyParameter<ECPrivateKey>(private));
    final sig = signer.generateSignature(digest) as ECSignature;
    signatures++;
    final body = [..._derInt(sig.r), ..._derInt(sig.s)];
    return Uint8List.fromList([0x30, body.length, ...body]);
  }

  bool verifyJws(String jwt) {
    final parts = jwt.split('.');
    final rs = _unb64url(parts[2]);
    if (rs.length != 64) return false;
    BigInt big(List<int> b) => BigInt.parse(b.map((e) => e.toRadixString(16).padLeft(2, '0')).join(), radix: 16);
    final verifier = ECDSASigner(null)..init(false, PublicKeyParameter<ECPublicKey>(public));
    return verifier.verifySignature(
        _sha256(ascii.encode('${parts[0]}.${parts[1]}')), ECSignature(big(rs.sublist(0, 32)), big(rs.sublist(32))));
  }
}

void main() {
  final app = AppKeys();
  setUpAll(() => print('core under test: ${NativeBackend.shared.description}'));

  test('without registration the core reports a domain error', () async {
    final signer = DpopSigner('dpop-key');
    await expectLater(signer.thumbprint(), throwsA(isA<ZeroErrorCrypto>()));
    signer.close();
  });

  group('registered', () {
    setUpAll(() => ZeroHost.register(app));
    tearDownAll(ZeroHost.unregister);

    test('thumbprint (libzero) equals RFC 7638 computed here', () async {
      final signer = DpopSigner('dpop-key');
      final json = '{"crv":"P-256","kty":"EC","x":"${_b64url(app.x)}","y":"${_b64url(app.y)}"}';
      expect(await signer.thumbprint(), _b64url(_sha256(utf8.encode(json))));
      signer.close();
    });

    test('DPoP proof is signed by the app key', () async {
      final signer = DpopSigner('dpop-key');
      final jwt = await signer.createProof('GET', 'https://zero.example/api', 'at-1', 'n-1');
      expect(app.verifyJws(jwt), isTrue);
      final parts = jwt.split('.');
      final header = jsonDecode(utf8.decode(_unb64url(parts[0]))) as Map<String, dynamic>;
      final claims = jsonDecode(utf8.decode(_unb64url(parts[1]))) as Map<String, dynamic>;
      expect(header['typ'], 'dpop+jwt');
      expect((header['jwk'] as Map)['x'], _b64url(app.x));
      expect(claims['htm'], 'GET');
      expect(claims['htu'], 'https://zero.example/api');
      expect(claims['ath'], _b64url(_sha256(utf8.encode('at-1'))));
      expect(claims['nonce'], 'n-1');
      signer.close();
    });

    test('concurrent proofs', () async {
      final signer = DpopSigner('dpop-key');
      final before = app.signatures;
      final proofs = await Future.wait(
          [for (var i = 0; i < 16; i++) signer.createProof('POST', 'https://zero.example/$i', null, null)]);
      expect(proofs.toSet().length, 16);
      expect(app.signatures - before, 16);
      expect(proofs.every(app.verifyJws), isTrue);
      signer.close();
    });

    test('app exception becomes the caller\'s ZeroError', () async {
      final signer = DpopSigner('unknown-key');
      await expectLater(signer.createProof('GET', 'https://zero.example/', null, null),
          throwsA(isA<ZeroErrorKeyNotFound>().having((e) => e.value, 'value', 'unknown-key')));
      signer.close();
    });

    test('invalid input is rejected by the core', () async {
      final signer = DpopSigner('dpop-key');
      await expectLater(signer.createProof('', 'ftp://x', null, null), throwsA(isA<ZeroErrorInvalidInput>()));
      signer.close();
    });
  });
}
