// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: executes contract calls through the Wabi native ABI
// with bun:ffi. No per-method code lives here.
//
// Threading (abi/ABI.md §6): the core completes futures and issues host calls
// on ITS threads. Both callbacks are `threadsafe` JSCallbacks: the core thread
// returns immediately and the JavaScript runs later on this thread's event
// loop. The callbacks carry no data except ids, so nothing races.
import { JSCallback, FFIType, toArrayBuffer, type Pointer } from "bun:ffi";
import { WabiCapabilities, WabiStatus, Contract } from "../abi_constants.ts";
import { NativeAbi } from "../native_abi.ts";
import type { ClientBackend } from "./backend.ts";
import { NativeStream, type StreamPull } from "./stream.ts";
import {
  WabiCancelledException,
  WabiIncompatibleLibraryException,
  WabiNativeException,
} from "./errors.ts";

const utf8 = new TextDecoder();

/** Identity and version of a loaded core, plus the compatibility rule. */
export class CoreInfo {
  constructor(
    readonly abiVersion: number,
    readonly capabilities: bigint,
    readonly contractId: string,
    readonly description: string,
  ) {}

  get abiMajor(): number {
    return this.abiVersion >>> 16;
  }

  get abiMinor(): number {
    return this.abiVersion & 0xffff;
  }

  /** Required capabilities; cancellation / threaded-callback are informative. */
  static readonly required =
    WabiCapabilities.syncCalls | WabiCapabilities.asyncCalls | WabiCapabilities.streams | WabiCapabilities.payloadMsgpack1;

  toString(): string {
    return `${this.description} (abi ${this.abiMajor}.${this.abiMinor}, ${this.contractId})`;
  }

  /** ABI: same major, core minor >= SDK minor. Contract: same package + major, core minor >= SDK minor. */
  ensureCompatible(): void {
    if (this.abiMajor !== WabiCapabilities.abiMajor || this.abiMinor < WabiCapabilities.abiMinor) {
      throw new WabiIncompatibleLibraryException(
        `native core '${this.description}' implements ABI ${this.abiMajor}.${this.abiMinor}, ` +
          `this SDK needs ${WabiCapabilities.abiMajor}.${WabiCapabilities.abiMinor}+`,
      );
    }
    const needed = CoreInfo.required | (WabiCapabilities.requiresHostCalls ? WabiCapabilities.hostCalls : 0n);
    if ((this.capabilities & needed) !== needed) {
      throw new WabiIncompatibleLibraryException(`native core '${this.description}' lacks required capabilities`);
    }
    const [pkg, ver = ""] = this.contractId.split("@");
    const [major, minor] = ver.split(".").map((p) => Number.parseInt(p, 10));
    if (pkg !== Contract.package || major !== Contract.major || minor === undefined || Number.isNaN(minor) || minor < Contract.minor) {
      throw new WabiIncompatibleLibraryException(
        `native core implements contract '${this.contractId}', this SDK was generated for '${Contract.id}'`,
      );
    }
  }
}

type Waiter = () => void;

export class NativeBackend implements ClientBackend {
  /** Environment variable with an explicit path to the core library. */
  static readonly libraryVariable = "WABI_NATIVE_LIB";

  /**
   * Set by a distribution package (e.g. the npm entry point) to locate the
   * library it ships for this platform. `WABI_NATIVE_LIB` still wins.
   */
  static libraryResolver: (() => string | undefined) | undefined;

  private static sharedInstance: NativeBackend | undefined;

  /** The backend for the platform's core library, validated on first use. */
  static get shared(): NativeBackend {
    return (NativeBackend.sharedInstance ??= new NativeBackend(NativeBackend.defaultPath()));
  }

  /** A backend bound to an explicit library. */
  static load(path: string): NativeBackend {
    return new NativeBackend(path);
  }

  /** `lib<name>.so`, `lib<name>.dylib` or `<name>.dll` (resolved by the OS loader). */
  static defaultPath(): string {
    const env = process.env[NativeBackend.libraryVariable];
    if (env) return env;
    const resolved = NativeBackend.libraryResolver?.();
    if (resolved) return resolved;
    const name = Contract.library;
    if (process.platform === "darwin") return `lib${name}.dylib`;
    if (process.platform === "win32") return `${name}.dll`;
    return `lib${name}.so`;
  }

  readonly info: CoreInfo;
  private readonly abi: NativeAbi;

  private constructor(readonly path: string) {
    try {
      this.abi = new NativeAbi(path);
    } catch (e) {
      throw new WabiIncompatibleLibraryException(`cannot load native core ${path}: ${(e as Error).message}`);
    }
    this.info = new CoreInfo(
      this.abi.wabi_abi_version(),
      this.abi.wabi_abi_capabilities(),
      this.abi.wabi_contract_id(),
      this.abi.wabi_core_info(),
    );
    this.info.ensureCompatible();
  }

  get description(): string {
    return `native[bun]: ${this.info}`;
  }

  // ------------------------------------------------------------ buffers

  /** Copies a core-owned buffer (`out` = [ptr, len]) onto the JS heap and frees it (always). */
  private consume(out: BigUint64Array): Uint8Array {
    const p = Number(out[0]);
    const n = Number(out[1]);
    try {
      if (p === 0 || n === 0) return new Uint8Array(0);
      return new Uint8Array(toArrayBuffer(p as Pointer, 0, n)).slice();
    } finally {
      if (p !== 0) this.abi.wabi_buffer_free(out);
    }
  }

  private error(status: number, detail?: Uint8Array): WabiNativeException {
    const name = this.abi.wabi_status_name(status);
    const text = detail && detail.length > 0 ? `${name}: ${utf8.decode(detail)}` : name;
    return new WabiNativeException(status, text);
  }

  // -------------------------------------------------------------- calls

  callSync(object: bigint, methodId: number, request: Uint8Array): Uint8Array {
    const out = new BigUint64Array(2);
    const status = this.abi.wabi_call_sync(object, methodId, request, out);
    const data = this.consume(out);
    if (status !== WabiStatus.ok) throw this.error(status, data);
    return data;
  }

  async callAsync(object: bigint, methodId: number, request: Uint8Array, signal?: AbortSignal): Promise<Uint8Array> {
    if (signal?.aborted) throw new WabiCancelledException(); // never start work for an aborted signal
    const out = new BigUint64Array(1);
    const status = this.abi.wabi_call_async(object, methodId, request, out);
    if (status !== WabiStatus.ok) throw this.error(status);
    const future = out[0]!;
    try {
      await this.whenReady(future, signal);
      const [taken, data] = this.take(future);
      if (taken === WabiStatus.ok) return data;
      if (taken === WabiStatus.errCancelled) throw new WabiCancelledException();
      throw this.error(taken, data);
    } finally {
      this.abi.wabi_future_drop(future);
    }
  }

  private take(future: bigint): [number, Uint8Array] {
    const out = new BigUint64Array(2);
    const status = this.abi.wabi_future_take(future, out);
    return [status, this.consume(out)];
  }

  // ------------------------------------------------------------ futures

  private readonly waiters = new Map<number, Waiter>();
  private nextKey = 1;
  private onReady: JSCallback | undefined;
  /** Keeps the event loop alive while a native completion is outstanding. */
  private keepAlive: ReturnType<typeof setInterval> | undefined;

  private readyCallback(): Pointer {
    this.onReady ??= new JSCallback(
      (userData: number) => {
        const waiter = this.waiters.get(userData);
        this.waiters.delete(userData);
        this.updateKeepAlive();
        waiter?.();
      },
      { args: [FFIType.ptr, FFIType.u64], returns: FFIType.void, threadsafe: true },
    );
    return this.onReady.ptr!;
  }

  private updateKeepAlive(): void {
    if (this.waiters.size > 0 && this.keepAlive === undefined) {
      this.keepAlive = setInterval(() => {}, 1 << 30);
    } else if (this.waiters.size === 0 && this.keepAlive !== undefined) {
      clearInterval(this.keepAlive);
      this.keepAlive = undefined;
    }
  }

  private async whenReady(future: bigint, signal?: AbortSignal): Promise<void> {
    const key = this.nextKey++;
    const ready = new Promise<void>((resolve) => this.waiters.set(key, resolve));
    this.updateKeepAlive();
    const status = this.abi.wabi_future_subscribe(future, this.readyCallback(), key as unknown as Pointer);
    if (status !== WabiStatus.ok) {
      this.waiters.delete(key); // the callback will never run
      this.updateKeepAlive();
      if (status === WabiStatus.ready) return;
      throw this.error(status);
    }
    // "completion wins" (ABI.md §4.1): after cancel the callback still fires
    // and take() reports what actually happened.
    const onAbort = () => this.abi.wabi_future_cancel(future);
    signal?.addEventListener("abort", onAbort, { once: true });
    try {
      await ready;
    } finally {
      signal?.removeEventListener("abort", onAbort);
    }
  }

  // ------------------------------------------------------------ streams

  private readonly pull: StreamPull = {
    next: async (stream) => {
      const out = new BigUint64Array(1);
      const status = this.abi.wabi_stream_next(stream, out);
      if (status !== WabiStatus.ok) throw this.error(status);
      const future = out[0]!;
      try {
        await this.whenReady(future);
        return this.take(future);
      } finally {
        this.abi.wabi_future_drop(future);
      }
    },
    cancel: (stream) => void this.abi.wabi_stream_cancel(stream),
    drop: (stream) => void this.abi.wabi_stream_drop(stream),
    error: (status, detail) => this.error(status, detail),
    okStatus: WabiStatus.ok,
    endStatuses: [WabiStatus.streamEnd, WabiStatus.errCancelled],
  };

  openStream(open: () => bigint, signal?: AbortSignal): AsyncIterableIterator<Uint8Array> {
    if (signal?.aborted) throw new WabiCancelledException();
    return new NativeStream(this.pull, open(), signal);
  }

  // ------------------------------------------------------------ objects

  dropObject(object: bigint): void {
    this.abi.wabi_object_drop(object);
  }

  // ------------------------------------------------- host calls (ABI 1.1)

  private host: JSCallback | undefined;

  /**
   * Installs `dispatch` (generated: `<Package>Host.register`) as the handler
   * for all WIT imports of this package (process-wide). The request stays
   * valid until the reply (ABI.md §4a), so copying it on the JS thread is
   * safe. Every call is answered exactly once, also when the app code throws.
   */
  registerHost(dispatch: (methodId: number, request: Uint8Array) => Promise<Uint8Array>): void {
    if (!this.abi.hasHostCalls || (this.info.capabilities & WabiCapabilities.hostCalls) === 0n) {
      throw new WabiIncompatibleLibraryException(`native core '${this.info.description}' does not support host calls (ABI 1.1)`);
    }
    const callback = new JSCallback(
      (_userData: number, methodId: number, request: number, length: number | bigint, call: bigint) => {
        const n = Number(length);
        const bytes = n === 0 ? new Uint8Array(0) : new Uint8Array(toArrayBuffer(request as Pointer, 0, n)).slice();
        void this.answer(dispatch, methodId, bytes, call);
      },
      { args: [FFIType.ptr, FFIType.u32, FFIType.ptr, FFIType.u64, FFIType.u64], returns: FFIType.void, threadsafe: true },
    );
    const status = this.abi.wabi_host_register(callback.ptr, null);
    if (status !== WabiStatus.ok) {
      callback.close();
      throw this.error(status);
    }
    this.host?.close();
    this.host = callback;
  }

  /** Removes the handler; later host calls fail inside the core. */
  unregisterHost(): void {
    if (this.abi.hasHostCalls) this.abi.wabi_host_register(null, null);
    this.host?.close();
    this.host = undefined;
  }

  private async answer(
    dispatch: (methodId: number, request: Uint8Array) => Promise<Uint8Array>,
    methodId: number,
    request: Uint8Array,
    call: bigint,
  ): Promise<void> {
    let status: number = WabiStatus.ok;
    let response: Uint8Array;
    try {
      response = await dispatch(methodId, request);
    } catch (e) {
      // WabiNativeException carries an ABI status; anything else is an app bug,
      // reported to the core as INTERNAL, never lost.
      status = e instanceof WabiNativeException ? e.status : WabiStatus.errInternal;
      response = new TextEncoder().encode(e instanceof Error ? `${e.name}: ${e.message}` : String(e));
    }
    this.abi.wabi_host_reply(call, status, response);
  }
}
