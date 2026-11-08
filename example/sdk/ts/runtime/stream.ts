// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: an ABI stream handle as an AsyncIterableIterator.
//
// Unlike an async generator, `return()` takes effect immediately even while a
// `next()` is pending: it cancels the producer in the core (the pending pull
// completes as cancelled, ABI.md §4) and releases the handle afterwards. So
// `break`, `iterator.return()` and an AbortSignal all stop a quiet stream.

export interface StreamPull {
  /** Starts the next pull; resolves to the ABI status and payload of `take`. */
  next(stream: bigint): Promise<[number, Uint8Array]>;
  cancel(stream: bigint): void;
  drop(stream: bigint): void;
  /** Maps a non-OK, non-terminal status to an exception. */
  error(status: number, detail: Uint8Array): Error;
  readonly okStatus: number;
  readonly endStatuses: readonly number[];
}

const finalizer = new FinalizationRegistry<{ pull: StreamPull; stream: bigint }>((s) => {
  try {
    s.pull.drop(s.stream);
  } catch {
    // never throw from a finalizer
  }
});

export class NativeStream implements AsyncIterableIterator<Uint8Array> {
  private done = false;
  private pending: Promise<IteratorResult<Uint8Array>> | undefined;
  private dropped = false;

  constructor(
    private readonly pull: StreamPull,
    private readonly stream: bigint,
    signal?: AbortSignal,
  ) {
    finalizer.register(this, { pull, stream }, this);
    if (signal?.aborted) void this.return();
    else signal?.addEventListener("abort", () => void this.return(), { once: true });
  }

  [Symbol.asyncIterator](): this {
    return this;
  }

  next(): Promise<IteratorResult<Uint8Array>> {
    if (this.done && !this.pending) return Promise.resolve({ done: true, value: undefined });
    // At most one pull at a time (ABI: WABI_ERR_BUSY otherwise): chain them.
    const run = (this.pending ?? Promise.resolve(undefined as unknown as IteratorResult<Uint8Array>)).then(() =>
      this.pullOne(),
    );
    const settled = run.finally(() => {
      if (this.pending === settled) this.pending = undefined;
      if (this.done) this.release();
    });
    this.pending = settled;
    return settled;
  }

  private async pullOne(): Promise<IteratorResult<Uint8Array>> {
    if (this.done) return { done: true, value: undefined };
    let status: number;
    let data: Uint8Array;
    try {
      [status, data] = await this.pull.next(this.stream);
    } catch (e) {
      this.done = true;
      throw e;
    }
    if (status === this.pull.okStatus && !this.done) return { done: false, value: data };
    this.done = true;
    if (status === this.pull.okStatus || this.pull.endStatuses.includes(status)) return { done: true, value: undefined };
    throw this.pull.error(status, data);
  }

  /** Stops the producer now; a pending `next()` resolves as done. Idempotent. */
  return(): Promise<IteratorResult<Uint8Array>> {
    if (!this.done) {
      this.done = true;
      this.pull.cancel(this.stream);
    }
    if (!this.pending) this.release();
    return Promise.resolve({ done: true, value: undefined });
  }

  private release(): void {
    if (this.dropped) return;
    this.dropped = true;
    finalizer.unregister(this);
    this.pull.drop(this.stream);
  }
}

/** Maps the items of a stream, forwarding `return()` (and so cancellation). */
export function mapStream<A, B>(source: AsyncIterableIterator<A>, map: (a: A) => B): AsyncIterableIterator<B> {
  const it: AsyncIterableIterator<B> = {
    async next(): Promise<IteratorResult<B>> {
      const r = await source.next();
      return r.done ? { done: true, value: undefined } : { done: false, value: map(r.value) };
    },
    async return(): Promise<IteratorResult<B>> {
      await source.return?.();
      return { done: true, value: undefined };
    },
    [Symbol.asyncIterator]() {
      return it;
    },
  };
  return it;
}
