// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic exception hierarchy.

/// Base class of every runtime-level exception of the Wabi SDK. Contract errors
/// (e.g. `ClientError`) are thrown directly and are not subclasses of this.
class WabiException implements Exception {
  WabiException(this.message);
  final String message;
  @override
  String toString() => '$runtimeType: $message';
}

/// The core broke the ABI contract or returned an undecodable payload.
class WabiProtocolException extends WabiException {
  WabiProtocolException(super.message);
}

/// An ABI-level failure (never a domain error), e.g. an invalid handle.
class WabiNativeException extends WabiException {
  WabiNativeException(this.status, super.message);
  final int status;
}

/// The native core is missing or implements an incompatible ABI/contract version.
class WabiIncompatibleLibraryException extends WabiException {
  WabiIncompatibleLibraryException(super.message);
}

/// A result error whose type is not a named contract type.
class WabiCallFailedException extends WabiException {
  WabiCallFailedException(this.error) : super('call failed: $error');
  final Object? error;
}

/// The operation was cancelled (via [CancellationToken] or by the core).
class WabiCancelledException extends WabiException {
  WabiCancelledException([super.message = 'operation was cancelled']);
}

/// The object was closed.
class WabiClosedException extends WabiException {
  WabiClosedException() : super('native object already closed');
}
