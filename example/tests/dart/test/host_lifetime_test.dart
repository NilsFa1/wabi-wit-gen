import 'dart:async';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import 'package:demo_client/demo_client.dart';
import 'package:test/test.dart';

void main() {
  final path = Platform.environment['WABI_HOST_PROBE'];
  test('retained native handler remains callable after unregister and replacement', () async {
    final libraryPath = path!;
    final lib = DynamicLibrary.open(libraryPath);
    final backend = NativeBackend.load(libraryPath);
    final snapshot = lib.lookupFunction<Void Function(), void Function()>('probe_snapshot');
    final invoke = lib.lookupFunction<Void Function(), void Function()>('probe_call_retained');
    final replies = lib.lookupFunction<Int32 Function(), int Function()>('probe_replies');
    final status = lib.lookupFunction<Int32 Function(), int Function()>('probe_status');
    final byte = lib.lookupFunction<Int32 Function(), int Function()>('probe_byte');

    Future<void> awaitReply() async {
      await Future<void>(() async {
        while (replies() == 0) { await Future<void>.delayed(const Duration(milliseconds: 1)); }
      }).timeout(const Duration(seconds: 5));
      expect(replies(), 1);
    }

    backend.registerHost((_, _, _) async => Uint8List.fromList([1]));
    snapshot(); // the core read its pointer but has not invoked it yet
    backend.unregisterHost();
    invoke();
    await awaitReply();
    expect(status(), -7); // errUnsupported, rather than jumping into freed memory

    backend.registerHost((_, _, _) async => Uint8List.fromList([1]));
    snapshot();
    backend.registerHost((_, _, _) async => Uint8List.fromList([2]));
    invoke(); // retained trampoline consults the current dispatcher
    await awaitReply();
    expect(status(), 0);
    expect(byte(), 2);
    backend.unregisterHost();
  }, skip: path == null ? 'build tests/abi/host_lifetime_probe.c and set WABI_HOST_PROBE' : false);
}
