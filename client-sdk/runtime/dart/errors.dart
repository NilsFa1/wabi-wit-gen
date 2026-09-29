// Hand-written, generic exception hierarchy.

/// Base class of every runtime-level exception of the Acme SDK. Contract errors
/// (e.g. `ClientError`) are thrown directly and are not subclasses of this.
class AcmeException implements Exception {
  AcmeException(this.message);
  final String message;
  @override
  String toString() => '$runtimeType: $message';
}

/// The core broke the ABI contract or returned an undecodable payload.
class AcmeProtocolException extends AcmeException {
  AcmeProtocolException(super.message);
}

/// An ABI-level failure (never a domain error), e.g. an invalid handle.
class AcmeNativeException extends AcmeException {
  AcmeNativeException(this.status, super.message);
  final int status;
}

/// The native core is missing or implements an incompatible ABI/contract version.
class AcmeIncompatibleLibraryException extends AcmeException {
  AcmeIncompatibleLibraryException(super.message);
}

/// A result error whose type is not a named contract type.
class AcmeCallFailedException extends AcmeException {
  AcmeCallFailedException(this.error) : super('call failed: $error');
  final Object? error;
}

/// The operation was cancelled (via [CancellationToken] or by the core).
class AcmeCancelledException extends AcmeException {
  AcmeCancelledException([super.message = 'operation was cancelled']);
}

/// The object was closed.
class AcmeClosedException extends AcmeException {
  AcmeClosedException() : super('native object already closed');
}
