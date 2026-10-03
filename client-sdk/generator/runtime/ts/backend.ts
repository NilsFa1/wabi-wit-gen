// Hand-written, generic: the seam between generated SDK code and whatever
// executes the contract (native core via bun:ffi today; a WebAssembly
// component or a test double later). Generated classes only see this.
import { WabiClosedException, WabiProtocolException } from "./errors.ts";

export interface ClientBackend {
  readonly description: string;

  /** Synchronous WIT function (never blocks on I/O). Returns the encoded result. */
  callSync(object: bigint, methodId: number, request: Uint8Array): Uint8Array;

  /** Async WIT function; aborting `signal` cancels it in the core. */
  callAsync(object: bigint, methodId: number, request: Uint8Array, signal?: AbortSignal): Promise<Uint8Array>;

  /**
   * Pull-based stream over the handle returned by `open` (called right away,
   * so the subscription is live before the caller awaits anything). `break`,
   * `return()` or aborting `signal` cancel and release the native stream, also
   * while a pull is pending.
   */
  openStream(open: () => bigint, signal?: AbortSignal): AsyncIterableIterator<Uint8Array>;

  dropObject(object: bigint): void;
}

const registry = new FinalizationRegistry<{ backend: ClientBackend; handle: bigint }>((r) => {
  try {
    r.backend.dropObject(r.handle);
  } catch {
    // never throw from a finalizer
  }
});

/** Owns one object handle; releases it exactly once (close() or garbage collection). */
export class ObjectHandle {
  private handle: bigint;

  constructor(owner: object, private readonly backend: ClientBackend, value: bigint) {
    if (value === 0n) throw new WabiProtocolException("core returned the null handle");
    this.handle = value;
    registry.register(owner, { backend, handle: value }, this);
  }

  get value(): bigint {
    if (this.handle === 0n) throw new WabiClosedException();
    return this.handle;
  }

  close(): void {
    const v = this.handle;
    if (v === 0n) return;
    this.handle = 0n;
    registry.unregister(this);
    this.backend.dropObject(v);
  }
}
