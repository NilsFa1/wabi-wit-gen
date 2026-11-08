// The app side of host calls in Dart/Flutter: the app owns the key (here a
// pointycastle key standing in for the Secure Enclave / Android Keystore),
// the core asks it to sign every request. The backend verifies the signatures.
// The core under test is chosen with WABI_NATIVE_LIB.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:pointycastle/export.dart';
import 'package:test/test.dart';
import 'package:demo_client/demo_client.dart';

final _curve = ECCurve_secp256r1();

SecureRandom _random() {
  final seed = Random.secure();
  return FortunaRandom()..seed(KeyParameter(Uint8List.fromList(List.generate(32, (_) => seed.nextInt(256)))));
}

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

AsymmetricKeyPair<PublicKey, PrivateKey> _newKey() =>
    (ECKeyGenerator()..init(ParametersWithRandom(ECKeyGeneratorParameters(_curve), _random()))).generateKeyPair();

/// The "keystore" of the app. Platform keystores return SPKI DER public keys
/// and ASN.1 DER signatures; this one does the same.
final class AppKeys implements HostKeys {
  final _key = _newKey();
  final _other = _newKey();
  var signatures = 0;

  /// Completed when the core gave up on a call for 'slow-key'.
  final cancelled = Completer<void>();

  /// SubjectPublicKeyInfo for id-ecPublicKey / prime256v1 + uncompressed point.
  Uint8List get spki {
    const prefix = '3059301306072a8648ce3d020106082a8648ce3d030107034200';
    final head = [for (var i = 0; i < prefix.length; i += 2) int.parse(prefix.substring(i, i + 2), radix: 16)];
    final q = (_key.publicKey as ECPublicKey).Q!;
    return Uint8List.fromList(
        [...head, 0x04, ..._unsigned(q.x!.toBigInteger()!, 32), ..._unsigned(q.y!.toBigInteger()!, 32)]);
  }

  String get fingerprint =>
      SHA256Digest().process(spki).map((b) => b.toRadixString(16).padLeft(2, '0')).join();

  // 'forged-key' announces the app's key but signs with another one.
  static bool _known(String keyId) => keyId == 'app-key' || keyId == 'forged-key';

  @override
  Future<Uint8List> publicKey(String keyId, CancellationToken cancellation) async {
    if (keyId == 'slow-key') { // e.g. waiting for the user: ends when the core no longer waits
      final waiting = Completer<Uint8List>();
      cancellation.register(() {
        cancelled.complete();
        waiting.completeError(WabiCancelledException());
      });
      return waiting.future;
    }
    if (!_known(keyId)) throw ClientErrorKeyError(keyId);
    return spki;
  }

  @override
  Future<Uint8List> sign(String keyId, Uint8List digest, CancellationToken cancellation) async {
    if (!_known(keyId)) throw ClientErrorKeyError(keyId);
    // e.g. a biometric prompt: answering later (and out of order) is fine
    await Future<void>.delayed(Duration(milliseconds: Random().nextInt(6)));
    final private = (keyId == 'app-key' ? _key : _other).privateKey as ECPrivateKey;
    final signer = ECDSASigner(null, HMac(SHA256Digest(), 64)) // RFC 6979, digest given by the core
      ..init(true, PrivateKeyParameter<ECPrivateKey>(private));
    final sig = signer.generateSignature(digest) as ECSignature;
    signatures++;
    final body = [..._derInt(sig.r), ..._derInt(sig.s)];
    return Uint8List.fromList([0x30, body.length, ...body]);
  }
}

/// Holds the first host call open until the test lets it continue.
final class BlockingKeys implements HostKeys {
  BlockingKeys(this.app);
  final AppKeys app;
  final entered = Completer<void>();
  final resume = Completer<void>();

  @override
  Future<Uint8List> publicKey(String keyId, CancellationToken cancellation) async {
    if (!entered.isCompleted) entered.complete();
    await resume.future;
    return app.publicKey(keyId, cancellation);
  }

  @override
  Future<Uint8List> sign(String keyId, Uint8List digest, CancellationToken cancellation) =>
      app.sign(keyId, digest, cancellation);
}

DemoClient signedClient(String keyId) => DemoClient(ClientOptions(
    endpoint: Platform.environment['WABI_TEST_ENDPOINT'] ?? 'mem://dart-host-tests',
    apiKey: 'demo-key',
    requestTimeoutMs: 10000,
    keyId: keyId));

void main() {
  final app = AppKeys();
  setUpAll(() => print('core under test: ${NativeBackend.shared.description}'));

  test('without registration the core reports a domain error', () async {
    final c = signedClient('app-key');
    await expectLater(c.connect(), throwsA(isA<ClientErrorKeyError>()));
    c.close();
  });

  test('an in-flight host call survives unregister and replacement', () async {
    final c = signedClient('app-key');
    final blocked = BlockingKeys(app);
    ClientHost.register(blocked);
    final pending = c.connect();
    try {
      await blocked.entered.future.timeout(const Duration(seconds: 5));
      ClientHost.unregister();
      ClientHost.register(app); // later calls go to the new registration
      blocked.resume.complete();
      await pending.timeout(const Duration(seconds: 5));
      expect(await c.keyFingerprint(), app.fingerprint);
    } finally {
      if (!blocked.resume.isCompleted) blocked.resume.complete();
      ClientHost.unregister();
      c.close();
    }
  });

  test('every instance can have its own keystore', () async {
    // No process-wide registration here.
    final first = AppKeys(), second = AppKeys();
    final a = signedClient('app-key'), b = signedClient('app-key'), c = signedClient('app-key');
    ClientHost.registerFor(a, first);
    ClientHost.registerFor(b, second);
    await Future.wait([a.connect(), b.connect()]);
    expect(await a.keyFingerprint(), first.fingerprint);
    expect(await b.keyFingerprint(), second.fingerprint);
    // An instance without its own registration falls back to the process-wide one (here: none).
    await expectLater(c.connect(), throwsA(isA<ClientErrorKeyError>()));
    // Unregistering one instance does not touch the other.
    ClientHost.unregisterFor(a);
    await expectLater(a.getCustomer('4711'), throwsA(isA<ClientErrorKeyError>()));
    expect((await b.getCustomer('4711')).id, '4711');
    for (final client in [a, b, c]) {
      client.close();
    }
  });

  group('registered', () {
    setUpAll(() => ClientHost.register(app));
    tearDownAll(ClientHost.unregister);

    test('the backend names the app key that signed the session', () async {
      final c = signedClient('app-key');
      await c.connect();
      expect(await c.keyFingerprint(), app.fingerprint);
      final before = app.signatures;
      expect((await c.getCustomer('4711')).name, 'Erika Mustermann');
      expect(app.signatures - before, 1, reason: 'one signature per request');
      c.close();
    });

    test('concurrent signed calls', () async {
      final c = signedClient('app-key');
      await c.connect();
      final before = app.signatures;
      final results = await Future.wait([
        for (var i = 0; i < 64; i++) c.sendCommand(Command(name: 'echo', payload: utf8.encode('payload-$i'))),
      ]);
      expect(app.signatures - before, 64);
      for (var i = 0; i < results.length; i++) { // every signature ended up on its own request
        expect(utf8.decode(results[i].payload), 'payload-$i');
      }
      c.close();
    });

    test('app exception becomes the caller\'s ClientError', () async {
      final c = signedClient('unknown-key');
      await expectLater(
          c.connect(), throwsA(isA<ClientErrorKeyError>().having((e) => e.value, 'value', 'unknown-key')));
      c.close();
    });

    test('the app learns that the core gave up on a call', () async {
      final c = signedClient('slow-key');
      await expectLater(c.connect(cancellationToken: CancellationToken.timeout(const Duration(milliseconds: 100))),
          throwsA(isA<WabiCancelledException>()));
      await app.cancelled.future.timeout(const Duration(seconds: 5));
      c.close();
    });

    test('a forged signature is rejected by the backend', () async {
      final c = signedClient('forged-key');
      await expectLater(c.connect(), throwsA(isA<ClientErrorUnauthorized>()));
      c.close();
    });
  });
}
