/// gematik:pki PoC SDK for Dart and Flutter.
///
/// `lib/src/generated` comes from `acme-wit-gen` (packages/pki/wit/pki.wit);
/// `lib/src/runtime` is the shared, contract-agnostic runtime.
library;

export 'src/generated/abi_constants.dart' show Contract;
export 'src/generated/gematik_certificate_verifier.dart';
export 'src/generated/types.dart';
export 'src/runtime/cancellation.dart';
export 'src/runtime/errors.dart';
export 'src/runtime/native_backend.dart' show CoreInfo, NativeBackend;
