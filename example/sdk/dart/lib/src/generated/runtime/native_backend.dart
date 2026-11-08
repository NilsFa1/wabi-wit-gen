// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: executes contract calls through the Wabi native ABI
// with dart:ffi. No per-method code lives here.
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import '../abi_constants.dart';
import '../native_abi.dart';
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
  static const required = WabiCapabilities.syncCalls |
      WabiCapabilities.asyncCalls |
      WabiCapabilities.streams |
      WabiCapabilities.payloadMsgpack1;

  @override
  String toString() => '$description (abi $abiMajor.$abiMinor, $contractId)';

  /// ABI: same major, core minor >= SDK minor. Contract: same package + major, core minor >= SDK minor.
  void ensureCompatible() {
    if (abiMajor != WabiCapabilities.abiMajor || abiMinor < WabiCapabilities.abiMinor) {
      throw WabiIncompatibleLibraryException("native core '$description' implements ABI $abiMajor.$abiMinor, "
          'this SDK needs ${WabiCapabilities.abiMajor}.${WabiCapabilities.abiMinor}+');
    }
    final needed = required | (WabiCapabilities.requiresHostCalls ? WabiCapabilities.hostCalls : 0);
    if (capabilities & needed != needed) {
      throw WabiIncompatibleLibraryException("native core '$description' lacks required capabilities");
    }
    final at = contractId.indexOf('@');
    final pkg = at < 0 ? contractId : contractId.substring(0, at);
    final ver = at < 0 ? const <String>[] : contractId.substring(at + 1).split('.');
    final major = ver.isNotEmpty ? int.tryParse(ver[0]) : null;
    final minor = ver.length > 1 ? int.tryParse(ver[1]) : null;
    if (pkg != Contract.package || major != Contract.major || minor == null || minor < Contract.minor) {
      throw WabiIncompatibleLibraryException(
          "native core implements contract '$contractId', this SDK was generated for '${Contract.id}'");
    }
  }

  /// Reads the metadata of an arbitrary core library without binding the SDK to it.
  /// Only the four metadata symbols are required, so even a library that is
  /// otherwise unusable can be diagnosed.
  static CoreInfo probe(String libraryPath) {
    final lib = NativeBackend._open(libraryPath);
    try {
      const p = Contract.symbolPrefix; // demo_client_abi_version, ... (ABI.md §1.1)
      final version = lib.lookupFunction<Uint32 Function(), int Function()>('${p}abi_version');
      final caps = lib.lookupFunction<Uint64 Function(), int Function()>('${p}abi_capabilities');
      final contract = lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('${p}contract_id');
      final core = lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('${p}core_info');
      return CoreInfo(version(), caps(), contract().toDartString(), core().toDartString());
    } on ArgumentError catch (e) {
      throw WabiIncompatibleLibraryException('$libraryPath is not an Wabi core: ${e.message}');
    }
  }
}

final class NativeBackend implements ClientBackend {
  NativeBackend._(this._abi) : info = _read(_abi) {
    info.ensureCompatible();
  }

  /// Environment variable with an explicit path to the core library.
  static const libraryVariable = 'WABI_NATIVE_LIB';

  static NativeBackend? _shared;

  /// The backend for the platform's core library, validated on first use.
  /// Looked up in this order:
  ///
  /// 1. `WABI_NATIVE_LIB` (explicit path, e.g. for tests against several cores),
  /// 2. the package's native code asset from `hook/build.dart` (Dart native
  ///    assets: Flutter and `dart run`/`dart test` bundle it automatically),
  ///    or the same symbols linked into the process (iOS xcframework),
  /// 3. the naming convention `libdemo_client.so` (Linux/Android jniLibs),
  ///    `demo_client.dll`, `libdemo_client.dylib` for manually bundled libraries.
  static NativeBackend get shared => _shared ??= NativeBackend._(_bindDefault());

  static NativeAbi _bindDefault() {
    final env = Platform.environment[libraryVariable];
    if (env != null && env.isNotEmpty) return _bind(env);
    ArgumentError assetError;
    try {
      return NativeAbi.asset();
    } on ArgumentError catch (e) {
      assetError = e; // no code asset and no symbols in the process: try the naming convention
    }
    try {
      return _bind(_conventionalPath());
    } on WabiIncompatibleLibraryException catch (e) {
      throw WabiIncompatibleLibraryException('${e.message}; code asset: ${assetError.message}');
    }
  }

  /// A backend bound to an explicit library.
  static NativeBackend load(String path) => NativeBackend._(_bind(path));

  static NativeAbi _bind(String? path) {
    final lib = _open(path);
    try {
      return NativeAbi(lib);
    } on ArgumentError catch (e) {
      throw WabiIncompatibleLibraryException('native core ${path ?? '(process)'} lacks ABI symbols: ${e.message}');
    }
  }

  static String? _conventionalPath() {
    if (Platform.isIOS) return null; // DynamicLibrary.process()
    const name = Contract.library; // derived from the WIT package, e.g. demo_client
    if (Platform.isMacOS) return 'lib$name.dylib';
    if (Platform.isWindows) return '$name.dll';
    return 'lib$name.so';
  }

  static DynamicLibrary _open(String? path) {
    try {
      return path == null ? DynamicLibrary.process() : DynamicLibrary.open(path);
    } on ArgumentError catch (e) {
      throw WabiIncompatibleLibraryException('cannot load native core ${path ?? '(process)'}: ${e.message}');
    }
  }

  static CoreInfo _read(NativeAbi abi) => CoreInfo(abi.wabi_abi_version(), abi.wabi_abi_capabilities(),
      abi.wabi_contract_id().toDartString(), abi.wabi_core_info().toDartString());

  final NativeAbi _abi;
  final CoreInfo info;

  @override
  String get description => 'native: $info';

  // ------------------------------------------------------------ buffers

  static Pointer<WabiBytes> _bytes(Arena arena, Uint8List data) {
    final s = arena<WabiBytes>();
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
  Uint8List _consume(WabiBuffer b) {
    try {
      if (b.len == 0 || b.ptr == nullptr) return Uint8List(0);
      return Uint8List.fromList(b.ptr.asTypedList(b.len));
    } finally {
      _abi.wabi_buffer_free(b);
    }
  }

  WabiNativeException _error(int status, [Uint8List? detail]) {
    final name = _abi.wabi_status_name(status).toDartString();
    final text = detail == null || detail.isEmpty ? name : '$name: ${utf8.decode(detail, allowMalformed: true)}';
    return WabiNativeException(status, text);
  }

  // -------------------------------------------------------------- calls

  @override
  Uint8List callSync(int object, int methodId, Uint8List request) => using((arena) {
        final out = arena<WabiBuffer>();
        final status = _abi.wabi_call_sync(object, methodId, _bytes(arena, request).ref, out);
        final data = _consume(out.ref);
        if (status != WabiStatus.ok) throw _error(status, data);
        return data;
      });

  @override
  Future<Uint8List> callAsync(int object, int methodId, Uint8List request,
      {CancellationToken? cancellationToken}) async {
    cancellationToken?.throwIfCancelled(); // never start work for a cancelled token
    final future = using((arena) {
      final out = arena<Uint64>();
      final status = _abi.wabi_call_async(object, methodId, _bytes(arena, request).ref, out);
      if (status != WabiStatus.ok) throw _error(status);
      return out.value;
    });
    try {
      await _whenReady(future, cancellationToken);
      final (status, data) = _take(future);
      return switch (status) {
        WabiStatus.ok => data,
        WabiStatus.errCancelled => throw WabiCancelledException(),
        _ => throw _error(status, data),
      };
    } finally {
      _abi.wabi_future_drop(future);
    }
  }

  (int, Uint8List) _take(int future) => using((arena) {
        final out = arena<WabiBuffer>();
        final status = _abi.wabi_future_take(future, out);
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
  static final _onReady = NativeCallable<WabiFutureCallback>.listener(_ready)..keepIsolateAlive = false;

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
    final status = _abi.wabi_future_subscribe(future, _onReady.nativeFunction, Pointer.fromAddress(key));
    if (status != WabiStatus.ok) {
      _waiters.remove(key); // the callback will never run
      if (_waiters.isEmpty) _onReady.keepIsolateAlive = false;
      if (status == WabiStatus.ready) return;
      throw _error(status);
    }
    final unregister = cancellationToken?.register(() => _abi.wabi_future_cancel(future));
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
      final status = _abi.wabi_stream_next(stream, out);
      if (status != WabiStatus.ok) throw _error(status);
      return out.value;
    });
    try {
      await _whenReady(future, null);
      return _take(future);
    } finally {
      _abi.wabi_future_drop(future);
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
        _abi.wabi_stream_drop(stream!);
      }
    }

    Future<void> pump() async {
      if (pulling || stream == null) return;
      pulling = true;
      try {
        while (!stopped && !controller.isPaused && !controller.isClosed) {
          final (status, data) = await _next(stream!);
          if (stopped) break;
          if (status == WabiStatus.ok) {
            controller.add(data);
          } else if (status == WabiStatus.streamEnd || status == WabiStatus.errCancelled) {
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
          _abi.wabi_stream_cancel(stream!); // a pending pull completes as cancelled
          if (!pulling) release();
        }
      },
    );
    return controller.stream;
  }

  // ------------------------------------------------------------ objects

  @override
  void dropObject(int object) {
    // An implementation registered for this object goes with it.
    if (_hostPerObject.remove(object) != null) _syncHost();
    _abi.wabi_object_drop(object);
  }

  // ------------------------------------------------- host calls (ABI 1.2)

  // Keep the trampolines alive for this isolate's lifetime: a core may have
  // copied their pointers before unregisterHost() or another registration.
  // The isolate must outlive all operations on this backend (including callbacks).
  NativeCallable<WabiHostHandler2>? _host;
  NativeCallable<WabiHostCancel>? _hostCancel;
  HostDispatch? _hostDispatch;
  final _hostPerObject = <int, HostDispatch>{};
  bool _hostRegistered = false;

  /// Calls the app is working on: cancelled when the core gives up on them.
  final _hostRunning = <int, CancellationToken>{};

  /// Installs [dispatch] (generated: `<Package>Host.register`) as the handler
  /// for the WIT imports of this package: for all of them, or, with [object],
  /// for the calls the core makes on behalf of that one object (it then takes
  /// precedence and goes away with the object). The core invokes the handler
  /// on one of ITS threads; NativeCallable.listener returns there immediately
  /// and runs the app code later on this isolate. The request stays valid
  /// until the reply (ABI.md §4a), so copying it here is safe. Every call is
  /// answered exactly once with wabi_host_reply, also when the app code throws.
  void registerHost(HostDispatch dispatch, {int? object}) {
    final previous = object == null ? _hostDispatch : _hostPerObject[object];
    void set(HostDispatch? d) {
      if (object == null) {
        _hostDispatch = d;
      } else if (d == null) {
        _hostPerObject.remove(object);
      } else {
        _hostPerObject[object] = d;
      }
    }

    set(dispatch);
    try {
      _syncHost();
    } catch (_) {
      set(previous);
      rethrow;
    }
  }

  /// Removes a dispatcher without freeing the native trampolines. Calls the
  /// core already started are still answered, with errUnsupported if necessary.
  void unregisterHost({int? object}) {
    if (object == null) {
      _hostDispatch = null;
    } else {
      _hostPerObject.remove(object);
    }
    _syncHost();
  }

  /// Registers with the core while there is any dispatcher, unregisters when the last one is gone.
  void _syncHost() {
    final wanted = _hostDispatch != null || _hostPerObject.isNotEmpty;
    if (wanted == _hostRegistered) return;
    final register = _abi.wabi_host_register2;
    if (!wanted) {
      _hostRegistered = false;
      final status = register?.call(nullptr, nullptr, nullptr);
      if (status != null && status != WabiStatus.ok) throw _error(status);
      return;
    }
    if (register == null || _abi.wabi_host_reply == null || info.capabilities & WabiCapabilities.hostCalls2 == 0) {
      throw WabiIncompatibleLibraryException("native core '${info.description}' does not support host calls (ABI 1.2)");
    }
    _host ??= NativeCallable<WabiHostHandler2>.listener(_onHostCall)..keepIsolateAlive = false;
    _hostCancel ??= NativeCallable<WabiHostCancel>.listener(_onHostCancel)..keepIsolateAlive = false;
    final status = register(_host!.nativeFunction, _hostCancel!.nativeFunction, nullptr);
    if (status != WabiStatus.ok) throw _error(status);
    _hostRegistered = true;
  }

  void _onHostCall(Pointer<Void> userData, int object, int methodId, Pointer<Uint8> request, int length, int call) {
    // capture once; an async call keeps its handler
    final dispatch = _hostPerObject[object] ?? _hostDispatch;
    final reply = _abi.wabi_host_reply!;
    if (dispatch == null) {
      using((arena) => reply(call, WabiStatus.errUnsupported,
          _bytes(arena, utf8.encode('no host handler registered')).ref));
      return;
    }
    final bytes = length == 0 ? Uint8List(0) : Uint8List.fromList(request.asTypedList(length));
    final cancellation = _hostRunning[call] = CancellationToken();
    _answer(dispatch, reply, methodId, bytes, call, cancellation).whenComplete(() => _hostRunning.remove(call));
  }

  /// The core no longer waits for [call]: tell the app code that is working on it.
  void _onHostCancel(Pointer<Void> userData, int call) => _hostRunning[call]?.cancel();

  static Future<void> _answer(HostDispatch dispatch, int Function(int, int, WabiBytes) reply, int methodId,
      Uint8List request, int call, CancellationToken cancellation) async {
    var status = WabiStatus.ok;
    Uint8List response;
    try {
      response = await dispatch(methodId, request, cancellation);
    } on WabiNativeException catch (e) {
      status = e.status;
      response = utf8.encode(e.message);
    } on WabiCancelledException {
      status = WabiStatus.errCancelled; // the core discards the answer anyway
      response = Uint8List(0);
    } catch (e) {
      status = WabiStatus.errInternal; // an app bug: reported to the core, never lost
      response = utf8.encode('$e');
    }
    using((arena) => reply(call, status, _bytes(arena, response).ref));
  }
}
