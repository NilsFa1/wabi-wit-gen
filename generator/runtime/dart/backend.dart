// Hand-written, generic: the seam between generated SDK code and whatever
// executes the contract (native core, WebAssembly component, test double).
import 'dart:async';
import 'dart:typed_data';

import 'cancellation.dart';
import 'errors.dart';

/// Runs one import function for the core (generated: `<Package>Host`): method
/// id and encoded parameters in, encoded return value out. [cancellation] is
/// cancelled when the core no longer waits for the answer.
typedef HostDispatch = Future<Uint8List> Function(int methodId, Uint8List request, CancellationToken cancellation);

/// An SDK object that stands for an object in the core.
abstract interface class NativeObject {
  /// The object's handle in the core (throws once closed).
  int get nativeHandle;
}

abstract interface class ClientBackend {
  String get description;

  /// Synchronous WIT function (never blocks on I/O). Returns the encoded result.
  Uint8List callSync(int object, int methodId, Uint8List request);

  /// Async WIT function; [cancellationToken] cancels it in the core.
  Future<Uint8List> callAsync(int object, int methodId, Uint8List request, {CancellationToken? cancellationToken});

  /// Single-subscription stream over the handle returned by [open] (called on listen).
  Stream<Uint8List> openStream(int Function() open);

  void dropObject(int object);
}

/// Owns one object handle; releases it exactly once (close() or GC finalizer).
final class ObjectHandle {
  ObjectHandle(Object owner, this._backend, int value) : _value = value {
    if (value == 0) throw WabiProtocolException('core returned the null handle');
    _finalizer.attach(owner, _Release(_backend, value), detach: this);
  }

  static final Finalizer<_Release> _finalizer = Finalizer((r) => r.backend.dropObject(r.handle));

  final ClientBackend _backend;
  int _value;

  int get value => _value != 0 ? _value : throw WabiClosedException();

  /// Gives the handle away without dropping the object: it now belongs to someone else (the core).
  int detach() {
    final v = value;
    _value = 0;
    _finalizer.detach(this);
    return v;
  }

  void close() {
    final v = _value;
    if (v == 0) return;
    _value = 0;
    _finalizer.detach(this);
    _backend.dropObject(v);
  }
}

final class _Release {
  _Release(this.backend, this.handle);
  final ClientBackend backend;
  final int handle;
}

/// Placeholder for running the same contract as a WebAssembly component
/// (docs/COMPONENT_MODEL.md). Generated client classes stay identical.
final class ComponentModelBackend implements ClientBackend {
  @override
  String get description => 'component-model (not implemented in this PoC)';
  Never _unsupported() => throw UnsupportedError('component model backend is an extension point');
  @override
  Uint8List callSync(int object, int methodId, Uint8List request) => _unsupported();
  @override
  Future<Uint8List> callAsync(int object, int methodId, Uint8List request, {CancellationToken? cancellationToken}) =>
      _unsupported();
  @override
  Stream<Uint8List> openStream(int Function() open) => _unsupported();
  @override
  void dropObject(int object) {}
}
