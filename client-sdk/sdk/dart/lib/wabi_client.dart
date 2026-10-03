/// Wabi client SDK for Dart and Flutter.
///
/// `lib/src/generated` comes from `wabi-wit-gen` (types, client, dart:ffi
/// bindings); `lib/src/generated/runtime` is the contract-agnostic runtime (written by wabi-wit-gen).
library;

export 'src/generated/abi_constants.dart' show Contract;
export 'src/generated/wabi_client.dart';
export 'src/generated/types.dart';
export 'src/generated/runtime/backend.dart' show ClientBackend, ComponentModelBackend;
export 'src/generated/runtime/cancellation.dart';
export 'src/generated/runtime/errors.dart';
export 'src/generated/runtime/native_backend.dart' show CoreInfo, NativeBackend;
