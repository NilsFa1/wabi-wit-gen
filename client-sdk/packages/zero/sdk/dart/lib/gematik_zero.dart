/// gematik:zero PoC SDK for Dart and Flutter.
///
/// `lib/src/generated` comes from `wabi-wit-gen` (packages/zero/wit/zero.wit);
/// `lib/src/generated/runtime` is the contract-agnostic runtime (written by wabi-wit-gen).
library;

export 'src/generated/abi_constants.dart' show Contract;
export 'src/generated/gematik_dpop_signer.dart';
export 'src/generated/types.dart';
export 'src/generated/zero_host.dart';
export 'src/generated/runtime/cancellation.dart';
export 'src/generated/runtime/errors.dart';
export 'src/generated/runtime/native_backend.dart' show CoreInfo, NativeBackend;
