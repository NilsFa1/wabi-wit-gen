// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic exception hierarchy.

/**
 * Base class of every runtime-level exception of the Wabi SDK. Contract errors
 * (e.g. `ClientException`) derive from it as well, so `instanceof WabiException`
 * catches everything the SDK throws.
 */
export class WabiException extends Error {
  constructor(message: string) {
    super(message);
    this.name = new.target.name;
  }
}

/** The core broke the ABI contract or returned an undecodable payload. */
export class WabiProtocolException extends WabiException {}

/** An ABI-level failure (never a domain error), e.g. an invalid handle. */
export class WabiNativeException extends WabiException {
  constructor(readonly status: number, message: string) {
    super(message);
  }
}

/** The native core is missing or implements an incompatible ABI/contract version. */
export class WabiIncompatibleLibraryException extends WabiException {}

/** A result error whose type is not a named contract type. */
export class WabiCallFailedException extends WabiException {
  constructor(readonly error: unknown) {
    super(`call failed: ${String(error)}`);
  }
}

/** The operation was cancelled (via `AbortSignal` or by the core). */
export class WabiCancelledException extends WabiException {
  constructor(message = "operation was cancelled") {
    super(message);
  }
}

/** The object was closed. */
export class WabiClosedException extends WabiException {
  constructor() {
    super("native object already closed");
  }
}
