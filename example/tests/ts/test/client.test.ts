// Same scenarios as tests/csharp, tests/java and tests/dart. The core under
// test is chosen with WABI_NATIVE_LIB, the backend it talks to with
// WABI_TEST_ENDPOINT (default: the in-process mock). Host calls: host_keys.test.ts.
import { beforeAll, expect, test } from "bun:test";
import {
  ClientException,
  NativeBackend,
  Transport,
  WabiCancelledException,
  DemoClient,
  WabiClosedException,
  type ClientEvent,
  type ClientOptions,
} from "../../../sdk/ts/index.ts";

const endpoint = process.env.WABI_TEST_ENDPOINT ?? "mem://ts-tests";
const options = (o: Partial<ClientOptions> = {}): ClientOptions => ({ endpoint, apiKey: "demo-key", requestTimeoutMs: 10_000, ...o });
const utf8 = (s: string) => new TextEncoder().encode(s);

async function connected(): Promise<DemoClient> {
  const c = new DemoClient(options());
  await c.connect();
  return c;
}

/** The `client-error` a failing call was rejected with. */
async function errorOf(p: Promise<unknown>) {
  const e = await p.then(() => { throw new Error("expected a ClientException"); }, (e) => e);
  expect(e).toBeInstanceOf(ClientException);
  return (e as ClientException).error;
}

beforeAll(() => console.log(`core under test: ${NativeBackend.shared.description}`));

test("sync call", () => {
  using c = new DemoClient(options());
  expect(c.state()).toBe("disconnected");
});

test("async call", async () => {
  using c = await connected();
  expect(c.state()).toBe("connected");
  expect(await c.getCustomer("4711")).toEqual({ id: "4711", name: "Erika Mustermann", email: "erika@example.com" });
  await c.disconnect();
  expect(c.state()).toBe("disconnected");
});

test("async error", async () => {
  using c = await connected();
  expect(await errorOf(c.getCustomer("does-not-exist"))).toEqual({ tag: "not-found" });
  using bad = new DemoClient(options({ apiKey: "wrong-key" }));
  expect(await errorOf(bad.connect())).toEqual({ tag: "unauthorized" });
  using offline = new DemoClient(options({ endpoint: "http://127.0.0.1:1" }));
  expect((await errorOf(offline.connect())).tag).toBe("network-error");
  using notConnected = new DemoClient(options());
  expect(await errorOf(notConnected.getCustomer("4711"))).toEqual({ tag: "disconnected" });
  // A session without a key was not signed by anyone.
  expect(await errorOf(c.keyFingerprint())).toEqual({ tag: "unauthorized" });
});

test("cancellation", async () => {
  using c = await connected();
  const t0 = performance.now();
  const slow = c.sendCommand({ name: "sleep", payload: utf8("5000") }, AbortSignal.timeout(100));
  await expect(slow).rejects.toBeInstanceOf(WabiCancelledException);
  expect(performance.now() - t0).toBeLessThan(2000);
  // An already aborted signal: the call is never started.
  await expect(c.getCustomer("4711", AbortSignal.abort())).rejects.toBeInstanceOf(WabiCancelledException);
  expect((await c.getCustomer("4711")).id).toBe("4711"); // the client is still usable
});

test("stream", async () => {
  using c = await connected();
  const received: ClientEvent[] = [];
  const reader = (async () => {
    for await (const event of c.events(AbortSignal.timeout(10_000))) {
      received.push(event);
      if (received.length === 2) break;
    }
  })();
  await Bun.sleep(100); // let the subscription start
  await c.sendCommand({ name: "broadcast", payload: utf8("hello 👋") });
  const created = await c.createCustomer("Max Mustermann", undefined);
  await reader;
  expect(received).toEqual([
    { tag: "message-received", value: "hello 👋" },
    { tag: "customer-updated", value: created },
  ]);
});

test("stream cancellation", async () => {
  using c = await connected();
  const abort = new AbortController();
  const reader = (async () => { for await (const _ of c.events(abort.signal)) { /* nothing arrives */ } })();
  await Bun.sleep(100);
  abort.abort();
  await reader; // an aborted subscription simply ends
  // A new subscription still works after the old one was cancelled.
  const next = (async () => { for await (const event of c.events(AbortSignal.timeout(10_000))) return event; })();
  await Bun.sleep(100);
  await c.sendCommand({ name: "broadcast", payload: utf8("again") });
  expect(await next).toEqual({ tag: "message-received", value: "again" });
});

test("resources as parameters and results", async () => {
  using transport = new Transport(endpoint);
  using a = transport.openClient(options({ endpoint: "unused://" })); // a resource as result
  using b = new DemoClient(options());
  b.useTransport(transport); // lent for the call
  await Promise.all([a.connect(), b.connect()]);
  const before = transport.requestCount();
  await a.getCustomer("4711");
  await b.getCustomer("4711");
  expect(transport.requestCount()).toBe(before + 2n); // both clients share the transport

  // An owned parameter moves into the core: the SDK object is gone afterwards.
  const other = new Transport(endpoint);
  using c = other.openClient(options());
  await c.connect();
  const [mine, theirs] = [transport.requestCount(), other.requestCount()];
  transport.absorb(other);
  expect(transport.requestCount()).toBe(mine + theirs);
  expect(() => other.requestCount()).toThrow(WabiClosedException);
  other.close(); // harmless: nothing left to drop
});

test("object lifetime", async () => {
  const c = await connected();
  c[Symbol.dispose]();
  c[Symbol.dispose](); // idempotent
  expect(() => c.state()).toThrow(WabiClosedException);
  await expect(c.getCustomer("4711")).rejects.toBeInstanceOf(WabiClosedException);
});

test("concurrent calls", async () => {
  using c = await connected();
  const results = await Promise.all(Array.from({ length: 500 }, () => c.getCustomer("4711")));
  expect(results.every((r) => r.name === "Erika Mustermann")).toBe(true);
});

test("large payload", async () => {
  using c = await connected();
  const payload = new Uint8Array(4 * 1024 * 1024);
  for (let i = 0; i < payload.length; i++) payload[i] = (i * 31 + (i >> 8)) & 0xff;
  const result = await c.sendCommand({ name: "echo", payload });
  expect(Buffer.compare(result.payload, payload)).toBe(0);
});

test("unicode strings", async () => {
  using c = await connected();
  const name = "Jürgen Groß 张伟 Ölçü 🚀👩‍💻";
  const created = await c.createCustomer(name, "jürgen@exämple.de");
  expect(created.name).toBe(name);
  expect(await c.getCustomer(created.id)).toEqual(created);
});
