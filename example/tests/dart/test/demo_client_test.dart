// Same scenarios as tests/csharp and tests/java. The core under test is
// chosen with WABI_NATIVE_LIB, the backend it talks to with WABI_TEST_ENDPOINT
// (default: the in-process mock). Host calls: host_keys_test.dart.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:demo_client/demo_client.dart';
import 'package:demo_client/src/generated/abi_constants.dart';
import 'package:demo_client/src/generated/wire_codec.dart';
import 'package:demo_client/src/generated/runtime/msgpack.dart';
import 'package:test/test.dart';

final testEndpoint = Platform.environment['WABI_TEST_ENDPOINT'] ?? 'mem://dart-tests';

ClientOptions options({String key = 'demo-key', String? endpoint}) =>
    ClientOptions(endpoint: endpoint ?? testEndpoint, apiKey: key, requestTimeoutMs: 10000);

Future<DemoClient> connected() async {
  final c = DemoClient(options());
  await c.connect();
  return c;
}

Uint8List bytes(String s) => Uint8List.fromList(utf8.encode(s));

void main() {
  setUpAll(() => print('core under test: ${NativeBackend.shared.description}'));

  test('sync call', () {
    final c = DemoClient(options());
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
    await expectLater(DemoClient(options(key: 'wrong')).connect(), throwsA(isA<ClientErrorUnauthorized>()));
    await expectLater(DemoClient(options(endpoint: 'http://127.0.0.1:1')).connect(),
        throwsA(isA<ClientErrorNetworkError>().having((e) => e.value, 'message', isNotEmpty)));
    await expectLater(DemoClient(options()).getCustomer('4711'), throwsA(isA<ClientErrorDisconnected>()));
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
    await expectLater(slow, throwsA(isA<WabiCancelledException>()));
    expect(sw.elapsedMilliseconds, lessThan(2000));
    // Already-cancelled token: the call is never started.
    await expectLater(c.getCustomer('4711', cancellationToken: CancellationToken()..cancel()),
        throwsA(isA<WabiCancelledException>()));
    expect((await c.getCustomer('4711')).id, '4711');
    c.close();
  });

  test('stream', () async {
    final c = await connected();
    final received = c.events().take(2).toList();
    await Future<void>.delayed(const Duration(milliseconds: 100));
    await c.sendCommand(Command(name: 'broadcast', payload: bytes('hello 👋')));
    final created = await c.createCustomer('Max', null);
    final events = await received.timeout(const Duration(seconds: 10));
    expect(events, [const ClientEventMessageReceived('hello 👋'), ClientEventCustomerUpdated(created)]);
    c.close();
  });

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
  });

  test('resources as parameters and results', () async {
    final transport = Transport(testEndpoint);
    final a = transport.openClient(options(endpoint: 'unused://')); // a resource as result
    final b = DemoClient(options())..useTransport(transport); // lent for the call
    await Future.wait([a.connect(), b.connect()]);
    final before = transport.requestCount();
    await a.getCustomer('4711');
    await b.getCustomer('4711');
    expect(transport.requestCount(), before + 2, reason: 'both clients share the transport');

    // An owned parameter moves into the core: the SDK object is gone afterwards.
    final other = Transport(testEndpoint);
    final c = other.openClient(options());
    await c.connect();
    final mine = transport.requestCount(), theirs = other.requestCount();
    transport.absorb(other);
    expect(transport.requestCount(), mine + theirs);
    expect(other.requestCount, throwsA(isA<WabiClosedException>()));
    other.close(); // harmless: nothing left to drop
    for (final client in [a, b, c]) {
      client.close();
    }
    transport.close();
  });

  test('object lifetime', () async {
    final c = await connected();
    c.close();
    expect(() => c.state(), throwsA(isA<WabiClosedException>()));
    await expectLater(c.getCustomer('4711'), throwsA(isA<WabiClosedException>()));
    for (var i = 0; i < 50; i++) {
      DemoClient(options()); // released by the finalizer eventually
    }
  });

  test('double drop', () {
    final c = DemoClient(options());
    c.close();
    c.close(); // idempotent at SDK level
    final backend = NativeBackend.shared;
    final w = MsgPackWriter()..writeArrayHeader(1);
    WireCodec.writeClientOptions(w, options());
    final handle = MsgPackReader(backend.callSync(0, MethodIds.clientConstructor, w.takeBytes())).readUint();
    backend.dropObject(handle);
    expect(() => backend.callSync(handle, MethodIds.clientState, Uint8List.fromList([0x90])),
        throwsA(isA<WabiNativeException>().having((e) => e.status, 'status', WabiStatus.errInvalidHandle)));
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
    await expectLater(slow, throwsA(anyOf(isA<ClientErrorDisconnected>(), isA<WabiCancelledException>())));
    await events.timeout(const Duration(seconds: 5)); // stream ends after close
  });

  test('core version mismatch', () {
    final info = NativeBackend.shared.info;
    info.ensureCompatible();
    final dir = Platform.environment['WABI_FAKE_CORES_DIR'];
    if (dir != null) {
      for (final f in ['libfake_abi_v2.so', 'libfake_contract_v2.so', 'libfake_no_streams.so']) {
        expect(() => CoreInfo.probe('$dir/$f').ensureCompatible(), throwsA(isA<WabiIncompatibleLibraryException>()));
      }
    }
    expect(() => CoreInfo(2 << 16, info.capabilities, info.contractId, 'v2').ensureCompatible(),
        throwsA(isA<WabiIncompatibleLibraryException>()));
    CoreInfo((1 << 16) | 4, info.capabilities, 'demo:client@1.3.0', 'newer minor').ensureCompatible();
  });

  test('large payload', () async {
    final c = await connected();
    final rnd = Random(42);
    final payload = Uint8List.fromList(List.generate(4 << 20, (_) => rnd.nextInt(256)));
    final result = await c.sendCommand(Command(name: 'echo', payload: payload));
    expect(result.payload, payload);
    c.close();
  });

  test('unicode strings', () async {
    final c = await connected();
    const name = 'Jürgen Groß 张伟 Ölçü 🚀👩‍💻';
    final created = await c.createCustomer(name, 'jürgen@exämple.de');
    expect(created.name, name);
    expect(await c.getCustomer(created.id), created);
    await expectLater(() => c.getCustomer('\ud800'), throwsArgumentError); // lone surrogate
    c.close();
  });
}
