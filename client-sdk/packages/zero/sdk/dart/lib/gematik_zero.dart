/// gematik:zero PoC SDK for Dart and Flutter.
///
/// `lib/src/generated` comes from `acme-wit-gen` (packages/zero/wit/zero.wit);
/// `lib/src/runtime` is the shared, contract-agnostic runtime.
library;

export 'src/generated/abi_constants.dart' show Contract;
export 'src/generated/gematik_dpop_signer.dart';
export 'src/generated/types.dart';
export 'src/generated/zero_host.dart';
export 'src/runtime/cancellation.dart';
export 'src/runtime/errors.dart';
export 'src/runtime/native_backend.dart' show CoreInfo, NativeBackend;
