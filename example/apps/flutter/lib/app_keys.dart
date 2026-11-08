// The app's keystore for the example: an EC P-256 key that never leaves the
// app. The core asks for its public key and for signatures through host calls.
// Here the key lives in memory (pointycastle); a real app would keep it in the
// Secure Enclave / Android Keystore and answer in the same formats.
import 'dart:math';
import 'dart:typed_data';

import 'package:pointycastle/export.dart';
import 'package:demo_client/demo_client.dart';

final class AppKeys implements HostKeys {
  AppKeys() {
    final seed = Random.secure();
    final random = FortunaRandom()
      ..seed(KeyParameter(Uint8List.fromList(List.generate(32, (_) => seed.nextInt(256)))));
    final pair = (ECKeyGenerator()..init(ParametersWithRandom(ECKeyGeneratorParameters(ECCurve_secp256r1()), random)))
        .generateKeyPair();
    _private = pair.privateKey as ECPrivateKey;
    _public = pair.publicKey as ECPublicKey;
  }

  late final ECPrivateKey _private;
  late final ECPublicKey _public;

  static Uint8List _bytes(BigInt v, [int? width]) {
    var hex = v.toRadixString(16);
    if (hex.length.isOdd) hex = '0$hex';
    final out = [for (var i = 0; i < hex.length; i += 2) int.parse(hex.substring(i, i + 2), radix: 16)];
    return Uint8List.fromList([...List.filled((width ?? out.length) - out.length, 0), ...out]);
  }

  static List<int> _derInt(BigInt v) {
    final b = _bytes(v);
    return [0x02, b.length + (b[0] >> 7), if (b[0] & 0x80 != 0) 0, ...b];
  }

  /// SubjectPublicKeyInfo (DER): header for id-ecPublicKey / prime256v1 + 0x04 || X || Y.
  @override
  Future<Uint8List> publicKey(String keyId, CancellationToken cancellation) async {
    const header = [0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00];
    final q = _public.Q!;
    return Uint8List.fromList([...header, 0x04, ..._bytes(q.x!.toBigInteger()!, 32), ..._bytes(q.y!.toBigInteger()!, 32)]);
  }

  /// ECDSA signature (ASN.1 DER) over the SHA-256 digest the core computed.
  @override
  Future<Uint8List> sign(String keyId, Uint8List digest, CancellationToken cancellation) async {
    final signer = ECDSASigner(null, HMac(SHA256Digest(), 64))..init(true, PrivateKeyParameter<ECPrivateKey>(_private));
    final sig = signer.generateSignature(digest) as ECSignature;
    final body = [..._derInt(sig.r), ..._derInt(sig.s)];
    return Uint8List.fromList([0x30, body.length, ...body]);
  }
}
