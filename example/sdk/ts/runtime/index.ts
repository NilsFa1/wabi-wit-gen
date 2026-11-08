// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic runtime of the TypeScript SDKs (Bun). Nothing here
// knows any contract type; generated code imports everything from this file.
export { MsgPackReader, MsgPackWriter } from "./msgpack.ts";
export {
  WabiCallFailedException,
  WabiCancelledException,
  WabiClosedException,
  WabiException,
  WabiIncompatibleLibraryException,
  WabiNativeException,
  WabiProtocolException,
} from "./errors.ts";
export { ObjectHandle, type ClientBackend, type HostDispatch, type NativeObject } from "./backend.ts";
export { CoreInfo, NativeBackend } from "./native_backend.ts";
export { mapStream, NativeStream } from "./stream.ts";
export { describeValue } from "./describe.ts";
