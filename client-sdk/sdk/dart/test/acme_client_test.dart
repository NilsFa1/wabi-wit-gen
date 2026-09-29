// Same scenarios as tests/csharp and tests/java. The core under test is
// chosen with ACME_NATIVE_LIB; ACME_TEST_PROFILE=basic skips tests that need
// a full backend (events, long-running commands) — used for the Zig stub.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:acme_client/acme_client.dart';
import 'package:acme_client/src/generated/abi_constants.dart';
import 'package:acme_client/src/generated/wire_codec.dart';
import 'package:acme_client/src/runtime/msgpack.dart';
import 'package:test/test.dart';

final basic = Platform.environment['ACME_TEST_PROFILE'] == 'basic';
final fullOnly = basic ? 'needs a full core (ACME_TEST_PROFILE=basic)' : null;

ClientOptions options({String key = 'demo-key', String endpoint = 'mem://dart-tests'}) =>
    ClientOptions(endpoint: endpoint, apiKey: key, requestTimeoutMs: 10000);

Future<AcmeClient> connected() async {
  final c = AcmeClient(options());
  await c.connect();
  return c;
}

Uint8List bytes(String s) => Uint8List.fromList(utf8.encode(s));

void main() {
  setUpAll(() => print('core under test: ${NativeBackend.shared.description}'));

  test('sync call', () {
    final c = AcmeClient(options());
    expect(c.state(), ConnectionState.disconnected);
    c.close();
  });

  test('async call', () async {
    final c = await connected();
    expect(c.state(), ConnectionState.connected);
    expect(await c.getCustomer('4711'),
        const Customer(id: '4711', name: 'Erika Mustermann', email: 'erika@example.com'));
    await c.disconnect();
    expect(c.state(), ConnectionState.disconnected);
    c.close();
  });

  test('async error', () async {
    final c = await connected();
    await expectLater(c.getCustomer('does-not-exist'), throwsA(isA<ClientErrorNotFound>()));
    await expectLater(AcmeClient(options(key: 'wrong')).connect(), throwsA(isA<ClientErrorUnauthorized>()));
    await expectLater(AcmeClient(options(endpoint: 'mem://unreachable')).connect(),
        throwsA(isA<ClientErrorNetworkError>().having((e) => e.value, 'message', isNotEmpty)));
    await expectLater(AcmeClient(options()).getCustomer('4711'), throwsA(isA<ClientErrorDisconnected>()));
    // Idiomatic handling with Dart 3 pattern matching:
    try {
      await c.getCustomer('nope');
    } on ClientError catch (e) {
      expect(switch (e) { ClientErrorNotFound() => 'not found', _ => 'other' }, 'not found');
    }
    c.close();
  });

  test('cancellation', () async {
    final c = await connected();
    final token = CancellationToken();
    final sw = Stopwatch()..start();
    final slow = c.sendCommand(Command(name: 'sleep', payload: bytes('5000')), cancellationToken: token);
    Timer(const Duration(milliseconds: 50), token.cancel);
    await expectLater(slow, throwsA(isA<AcmeCancelledException>()));
    expect(sw.elapsedMilliseconds, lessThan(2000));
    // Already-cancelled token: the call is never started.
    await expectLater(c.getCustomer('4711', cancellationToken: CancellationToken()..cancel()),
        throwsA(isA<AcmeCancelledException>()));
    expect((await c.getCustomer('4711')).id, '4711');
    c.close();
  }, skip: fullOnly);

  test('stream', () async {
    final c = await connected();
    final received = c.events().take(2).toList();
    await Future<void>.delayed(const Duration(milliseconds: 100));
    await c.sendCommand(Command(name: 'broadcast', payload: bytes('hello 👋')));
    final created = await c.createCustomer('Max', null);
    final events = await received.timeout(const Duration(seconds: 10));
    expect(events, [const ClientEventMessageReceived('hello 👋'), ClientEventCustomerUpdated(created)]);
    c.close();
  }, skip: fullOnly);

  test('stream cancellation', () async {
    final c = await connected();
    final first = <ClientEvent>[];
    final sub = c.events().listen(first.add);
    await Future<void>.delayed(const Duration(milliseconds: 50));
    await sub.cancel();
    // A new subscription works; the cancelled one receives nothing more.
    final next = c.events().first;
    await Future<void>.delayed(const Duration(milliseconds: 100));
    await c.sendCommand(Command(name: 'broadcast', payload: bytes('again')));
    expect(await next.timeout(const Duration(seconds: 10)), const ClientEventMessageReceived('again'));
    expect(first, isEmpty);
    // `break` in await-for also releases the native stream.
    final breaker = () async {
      await for (final e in c.events()) {
        return e;
      }
      return null;
    }();
    await Future<void>.delayed(const Duration(milliseconds: 100));
    await c.sendCommand(Command(name: 'broadcast', payload: bytes('x')));
    expect(await breaker, isNotNull);
    c.close();
  }, skip: fullOnly);

  test('object lifetime', () async {
    final c = await connected();
    c.close();
    expect(() => c.state(), throwsA(isA<AcmeClosedException>()));
    await expectLater(c.getCustomer('4711'), throwsA(isA<AcmeClosedException>()));
    for (var i = 0; i < 50; i++) {
      AcmeClient(options()); // released by the finalizer eventually
    }
  });

  test('double drop', () {
    final c = AcmeClient(options());
    c.close();
    c.close(); // idempotent at SDK level
    final backend = NativeBackend.shared;
    final w = MsgPackWriter()..writeArrayHeader(1);
    WireCodec.writeClientOptions(w, options());
    final handle = MsgPackReader(backend.callSync(0, MethodIds.clientConstructor, w.takeBytes())).readUint();
    backend.dropObject(handle);
    expect(() => backend.callSync(handle, MethodIds.clientState, Uint8List.fromList([0x90])),
        throwsA(isA<AcmeNativeException>().having((e) => e.status, 'status', AcmeStatus.errInvalidHandle)));
    backend.dropObject(handle); // second drop: detected by the core, harmless
  });

  test('concurrent calls', () async {
    final c = await connected();
    final results = await Future.wait([for (var i = 0; i < 500; i++) c.getCustomer('4711')]);
    expect(results.every((r) => r.name == 'Erika Mustermann'), isTrue);
    c.close();
  });

  test('callback during dispose', () async {
    final c = await connected();
    final slow = c.sendCommand(Command(name: 'sleep', payload: bytes('3000')));
    final events = c.events().toList();
    await Future<void>.delayed(const Duration(milliseconds: 100));
    c.close();
    await expectLater(slow, throwsA(anyOf(isA<ClientErrorDisconnected>(), isA<AcmeCancelledException>())));
    await events.timeout(const Duration(seconds: 5)); // stream ends after close
  }, skip: fullOnly);

  test('core version mismatch', () {
    final info = NativeBackend.shared.info;
    info.ensureCompatible();
    final dir = Platform.environment['ACME_FAKE_CORES_DIR'];
    if (dir != null) {
      for (final f in ['libfake_abi_v2.so', 'libfake_contract_v2.so', 'libfake_no_streams.so']) {
        expect(() => CoreInfo.probe('$dir/$f').ensureCompatible(), throwsA(isA<AcmeIncompatibleLibraryException>()));
      }
    }
    expect(() => CoreInfo(2 << 16, info.capabilities, info.contractId, 'v2').ensureCompatible(),
        throwsA(isA<AcmeIncompatibleLibraryException>()));
    CoreInfo((1 << 16) | 4, info.capabilities, 'acme:client@1.3.0', 'newer minor').ensureCompatible();
  });

  test('large payload', () async {
    final c = await connected();
    final rnd = Random(42);
    final payload = Uint8List.fromList(List.generate(4 << 20, (_) => rnd.nextInt(256)));
    final result = await c.sendCommand(Command(name: 'echo', payload: payload));
    expect(result.payload, payload);
    c.close();
  }, skip: fullOnly);

  test('unicode strings', () async {
    final c = await connected();
    const name = 'Jürgen Groß 张伟 Ölçü 🚀👩‍💻';
    final created = await c.createCustomer(name, 'jürgen@exämple.de');
    expect(created.name, name);
    expect(await c.getCustomer(created.id), created);
    await expectLater(() => c.getCustomer('\ud800'), throwsArgumentError); // lone surrogate
    c.close();
  }, skip: fullOnly);
}
