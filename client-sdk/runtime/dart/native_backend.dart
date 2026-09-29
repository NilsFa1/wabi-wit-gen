// Hand-written, generic: executes contract calls through the Acme native ABI
// with dart:ffi. No per-method code lives here.
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import 'package:acme_client/src/generated/abi_constants.dart';
import 'package:acme_client/src/generated/native_abi.dart';
import 'package:ffi/ffi.dart';

import 'backend.dart';
import 'cancellation.dart';
import 'errors.dart';

/// Identity and version of a loaded core, plus the compatibility rule.
final class CoreInfo {
  CoreInfo(this.abiVersion, this.capabilities, this.contractId, this.description);

  final int abiVersion;
  final int capabilities;
  final String contractId;
  final String description;

  int get abiMajor => abiVersion >> 16;
  int get abiMinor => abiVersion & 0xffff;

  /// Required capabilities; cancellation / threaded-callback are informative.
  static const required = AcmeCapabilities.syncCalls |
      AcmeCapabilities.asyncCalls |
      AcmeCapabilities.streams |
      AcmeCapabilities.payloadMsgpack1;

  @override
  String toString() => '$description (abi $abiMajor.$abiMinor, $contractId)';

  /// ABI: same major, core minor >= SDK minor. Contract: same package + major, core minor >= SDK minor.
  void ensureCompatible() {
    if (abiMajor != AcmeCapabilities.abiMajor || abiMinor < AcmeCapabilities.abiMinor) {
      throw AcmeIncompatibleLibraryException("native core '$description' implements ABI $abiMajor.$abiMinor, "
          'this SDK needs ${AcmeCapabilities.abiMajor}.${AcmeCapabilities.abiMinor}+');
    }
    if (capabilities & required != required) {
      throw AcmeIncompatibleLibraryException("native core '$description' lacks required capabilities");
    }
    final at = contractId.indexOf('@');
    final pkg = at < 0 ? contractId : contractId.substring(0, at);
    final ver = at < 0 ? const <String>[] : contractId.substring(at + 1).split('.');
    final major = ver.isNotEmpty ? int.tryParse(ver[0]) : null;
    final minor = ver.length > 1 ? int.tryParse(ver[1]) : null;
    if (pkg != Contract.package || major != Contract.major || minor == null || minor < Contract.minor) {
      throw AcmeIncompatibleLibraryException(
          "native core implements contract '$contractId', this SDK was generated for '${Contract.id}'");
    }
  }

  /// Reads the metadata of an arbitrary core library without binding the SDK to it.
  /// Only the four metadata symbols are required, so even a library that is
  /// otherwise unusable can be diagnosed.
  static CoreInfo probe(String libraryPath) {
    final lib = NativeBackend._open(libraryPath);
    try {
      final version = lib.lookupFunction<Uint32 Function(), int Function()>('acme_abi_version');
      final caps = lib.lookupFunction<Uint64 Function(), int Function()>('acme_abi_capabilities');
      final contract = lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('acme_contract_id');
      final core = lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('acme_core_info');
      return CoreInfo(version(), caps(), contract().toDartString(), core().toDartString());
    } on ArgumentError catch (e) {
      throw AcmeIncompatibleLibraryException('$libraryPath is not an Acme core: ${e.message}');
    }
  }
}

final class NativeBackend implements ClientBackend {
  NativeBackend._(this._abi) : info = _read(_abi) {
    info.ensureCompatible();
  }

  /// Environment variable with an explicit path to the core library.
  static const libraryVariable = 'ACME_NATIVE_LIB';

  static NativeBackend? _shared;

  /// The backend for the platform's core library, validated on first use:
  /// `libacme_client.so` (Linux/Android, e.g. from jniLibs), `acme_client.dll`,
  /// `libacme_client.dylib`, and on iOS the statically linked xcframework.
  static NativeBackend get shared => _shared ??= NativeBackend._(_bind(_defaultPath()));

  /// A backend bound to an explicit library.
  static NativeBackend load(String path) => NativeBackend._(_bind(path));

  static NativeAbi _bind(String? path) {
    final lib = _open(path);
    try {
      return NativeAbi(lib);
    } on ArgumentError catch (e) {
      throw AcmeIncompatibleLibraryException('native core ${path ?? '(process)'} lacks ABI symbols: ${e.message}');
    }
  }

  static String? _defaultPath() {
    final env = Platform.environment[libraryVariable];
    if (env != null && env.isNotEmpty) return env;
    if (Platform.isIOS) return null; // DynamicLibrary.process()
    if (Platform.isMacOS) return 'libacme_client.dylib';
    if (Platform.isWindows) return 'acme_client.dll';
    return 'libacme_client.so';
  }

  static DynamicLibrary _open(String? path) {
    try {
      return path == null ? DynamicLibrary.process() : DynamicLibrary.open(path);
    } on ArgumentError catch (e) {
      throw AcmeIncompatibleLibraryException('cannot load native core ${path ?? '(process)'}: ${e.message}');
    }
  }

  static CoreInfo _read(NativeAbi abi) => CoreInfo(abi.acme_abi_version(), abi.acme_abi_capabilities(),
      abi.acme_contract_id().toDartString(), abi.acme_core_info().toDartString());

  final NativeAbi _abi;
  final CoreInfo info;

  @override
  String get description => 'native: $info';

  // ------------------------------------------------------------ buffers

  static Pointer<AcmeBytes> _bytes(Arena arena, Uint8List data) {
    final s = arena<AcmeBytes>();
    if (data.isEmpty) {
      s.ref.ptr = nullptr;
    } else {
      final p = arena<Uint8>(data.length);
      p.asTypedList(data.length).setAll(0, data);
      s.ref.ptr = p;
    }
    s.ref.len = data.length;
    return s;
  }

  /// Copies a core-owned buffer onto the Dart heap and frees it (always).
  Uint8List _consume(AcmeBuffer b) {
    try {
      if (b.len == 0 || b.ptr == nullptr) return Uint8List(0);
      return Uint8List.fromList(b.ptr.asTypedList(b.len));
    } finally {
      _abi.acme_buffer_free(b);
    }
  }

  AcmeNativeException _error(int status, [Uint8List? detail]) {
    final name = _abi.acme_status_name(status).toDartString();
    final text = detail == null || detail.isEmpty ? name : '$name: ${utf8.decode(detail, allowMalformed: true)}';
    return AcmeNativeException(status, text);
  }

  // -------------------------------------------------------------- calls

  @override
  Uint8List callSync(int object, int methodId, Uint8List request) => using((arena) {
        final out = arena<AcmeBuffer>();
        final status = _abi.acme_call_sync(object, methodId, _bytes(arena, request).ref, out);
        final data = _consume(out.ref);
        if (status != AcmeStatus.ok) throw _error(status, data);
        return data;
      });

  @override
  Future<Uint8List> callAsync(int object, int methodId, Uint8List request,
      {CancellationToken? cancellationToken}) async {
    cancellationToken?.throwIfCancelled(); // never start work for a cancelled token
    final future = using((arena) {
      final out = arena<Uint64>();
      final status = _abi.acme_call_async(object, methodId, _bytes(arena, request).ref, out);
      if (status != AcmeStatus.ok) throw _error(status);
      return out.value;
    });
    try {
      await _whenReady(future, cancellationToken);
      final (status, data) = _take(future);
      return switch (status) {
        AcmeStatus.ok => data,
        AcmeStatus.errCancelled => throw AcmeCancelledException(),
        _ => throw _error(status, data),
      };
    } finally {
      _abi.acme_future_drop(future);
    }
  }

  (int, Uint8List) _take(int future) => using((arena) {
        final out = arena<AcmeBuffer>();
        final status = _abi.acme_future_take(future, out);
        return (status, _consume(out.ref));
      });

  // ------------------------------------------------------------ futures

  /// Pending subscriptions of this isolate: user_data is a key into this map.
  static final _waiters = <int, Completer<void>>{};
  static var _nextKey = 0;

  /// Invoked by the core on one of ITS threads; NativeCallable.listener returns
  /// immediately there and runs [_ready] later on this isolate's event loop.
  /// No Flutter/UI thread is assumed: results arrive on the isolate that made
  /// the call, like any other Dart future.
  static final _onReady = NativeCallable<AcmeFutureCallback>.listener(_ready)..keepIsolateAlive = false;

  static void _ready(Pointer<Void> userData, int future) {
    final completer = _waiters.remove(userData.address);
    if (_waiters.isEmpty) _onReady.keepIsolateAlive = false;
    completer?.complete();
  }

  Future<void> _whenReady(int future, CancellationToken? cancellationToken) async {
    final key = ++_nextKey;
    final completer = Completer<void>();
    _waiters[key] = completer;
    _onReady.keepIsolateAlive = true; // a pending native completion keeps the isolate alive
    final status = _abi.acme_future_subscribe(future, _onReady.nativeFunction, Pointer.fromAddress(key));
    if (status != AcmeStatus.ok) {
      _waiters.remove(key); // the callback will never run
      if (_waiters.isEmpty) _onReady.keepIsolateAlive = false;
      if (status == AcmeStatus.ready) return;
      throw _error(status);
    }
    final unregister = cancellationToken?.register(() => _abi.acme_future_cancel(future));
    try {
      await completer.future;
    } finally {
      unregister?.call();
    }
  }

  // ------------------------------------------------------------ streams

  Future<(int, Uint8List)> _next(int stream) async {
    final future = using((arena) {
      final out = arena<Uint64>();
      final status = _abi.acme_stream_next(stream, out);
      if (status != AcmeStatus.ok) throw _error(status);
      return out.value;
    });
    try {
      await _whenReady(future, null);
      return _take(future);
    } finally {
      _abi.acme_future_drop(future);
    }
  }

  /// Pull-based: the core is only asked for the next item while the listener
  /// is subscribed and not paused (backpressure). Cancelling the subscription
  /// cancels and releases the native stream.
  @override
  Stream<Uint8List> openStream(int Function() open) {
    late final StreamController<Uint8List> controller;
    int? stream;
    var pulling = false;
    var stopped = false;
    var released = false;

    void release() {
      if (!released && stream != null) {
        released = true;
        _abi.acme_stream_drop(stream!);
      }
    }

    Future<void> pump() async {
      if (pulling || stream == null) return;
      pulling = true;
      try {
        while (!stopped && !controller.isPaused && !controller.isClosed) {
          final (status, data) = await _next(stream!);
          if (stopped) break;
          if (status == AcmeStatus.ok) {
            controller.add(data);
          } else if (status == AcmeStatus.streamEnd || status == AcmeStatus.errCancelled) {
            stopped = true;
            controller.close();
          } else {
            stopped = true;
            controller.addError(_error(status, data));
            controller.close();
          }
        }
      } catch (e, s) {
        if (!stopped) {
          stopped = true;
          controller.addError(e, s);
          controller.close();
        }
      } finally {
        pulling = false;
        if (stopped) release();
      }
    }

    controller = StreamController<Uint8List>(
      onListen: () {
        try {
          stream = open(); // the native subscription starts on listen
        } catch (e, s) {
          stopped = true;
          controller.addError(e, s);
          controller.close();
          return;
        }
        pump();
      },
      onResume: pump,
      onCancel: () {
        stopped = true;
        if (stream != null && !released) {
          _abi.acme_stream_cancel(stream!); // a pending pull completes as cancelled
          if (!pulling) release();
        }
      },
    );
    return controller.stream;
  }

  // ------------------------------------------------------------ objects

  @override
  void dropObject(int object) => _abi.acme_object_drop(object);
}
