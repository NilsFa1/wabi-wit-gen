/// Acme client SDK for Dart and Flutter.
///
/// `lib/src/generated` comes from `acme-wit-gen` (types, client, dart:ffi
/// bindings); `lib/src/runtime` is the hand-written, contract-agnostic runtime.
library;

export 'src/generated/abi_constants.dart' show Contract;
export 'src/generated/acme_client.dart';
export 'src/generated/types.dart';
export 'src/runtime/backend.dart' show ClientBackend, ComponentModelBackend;
export 'src/runtime/cancellation.dart';
export 'src/runtime/errors.dart';
export 'src/runtime/native_backend.dart' show CoreInfo, NativeBackend;
