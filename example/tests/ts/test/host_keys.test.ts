// The app side of host calls in TypeScript (Bun): the app owns the key (here
// @noble/curves standing in for a KMS/HSM; same SPKI and DER formats), the core
// asks it to sign every request. The backend verifies the signatures. The core
// under test is chosen with WABI_NATIVE_LIB. Same scenarios as the C#, Java,
// Dart and Swift tests.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { p256 } from "@noble/curves/p256";
import { ClientException, ClientHost, NativeBackend, WabiCancelledException, DemoClient, type HostKeys } from "../../../sdk/ts/index.ts";

/** SubjectPublicKeyInfo header for id-ecPublicKey / prime256v1 (+ uncompressed point). */
const SPKI_PREFIX = Buffer.from("3059301306072a8648ce3d020106082a8648ce3d030107034200", "hex");

// "forged-key" announces the app's key but signs with another one.
const known = (keyId: string) => keyId === "app-key" || keyId === "forged-key";

/** The "keystore" of the app. Platform keystores return SPKI DER public keys and ASN.1 DER signatures. */
class AppKeys implements HostKeys {
  readonly priv = p256.utils.randomPrivateKey();
  readonly other = p256.utils.randomPrivateKey();
  readonly spki = Buffer.concat([SPKI_PREFIX, p256.getPublicKey(this.priv, false)]); // 0x04 || X || Y
  signatures = 0;
  /** Resolved when the core gave up on a call for "slow-key". */
  readonly cancelled = Promise.withResolvers<void>();

  get fingerprint(): string {
    return createHash("sha256").update(this.spki).digest("hex");
  }

  publicKey(keyId: string, signal?: AbortSignal): Uint8Array | Promise<Uint8Array> {
    if (keyId === "slow-key") {
      // e.g. waiting for the user: ends when the core no longer waits
      return new Promise((_, reject) =>
        signal!.addEventListener("abort", () => {
          this.cancelled.resolve();
          reject(signal!.reason);
        }),
      );
    }
    if (!known(keyId)) throw new ClientException({ tag: "key-error", value: keyId });
    return this.spki;
  }

  async sign(keyId: string, digest: Uint8Array): Promise<Uint8Array> {
    if (!known(keyId)) throw new ClientException({ tag: "key-error", value: keyId });
    await Bun.sleep(Math.floor(Math.random() * 6)); // e.g. a biometric prompt: answering later (and out of order) is fine
    this.signatures++;
    return p256.sign(digest, keyId === "app-key" ? this.priv : this.other, { prehash: false }).toDERRawBytes();
  }
}

/** Holds the first public-key request until the test lets it go. */
class BlockingKeys implements HostKeys {
  readonly entered = Promise.withResolvers<void>();
  readonly resume = Promise.withResolvers<void>();
  constructor(readonly app: AppKeys) {}

  async publicKey(keyId: string): Promise<Uint8Array> {
    this.entered.resolve();
    await this.resume.promise;
    return this.app.publicKey(keyId);
  }

  sign(keyId: string, digest: Uint8Array) {
    return this.app.sign(keyId, digest);
  }
}

const app = new AppKeys();
const signedClient = (keyId: string) =>
  new DemoClient({ endpoint: process.env.WABI_TEST_ENDPOINT ?? "mem://ts-host-tests", apiKey: "demo-key", requestTimeoutMs: 10_000, keyId });
const within = <T>(p: Promise<T>, ms = 5000) =>
  Promise.race([p, Bun.sleep(ms).then(() => { throw new Error(`timeout after ${ms} ms`); })]);
const errorOf = async (p: Promise<unknown>) => {
  const e = await p.then(() => { throw new Error("expected a ClientException"); }, (e) => e);
  expect(e).toBeInstanceOf(ClientException);
  return (e as ClientException).error;
};

beforeAll(() => console.log(`core under test: ${NativeBackend.shared.description}`));

test("without registration the core reports a domain error", async () => {
  using c = signedClient("app-key");
  expect((await errorOf(c.connect())).tag).toBe("key-error");
});

test("an in-flight host call survives unregister and replacement", async () => {
  using c = signedClient("app-key");
  const blocked = new BlockingKeys(app);
  ClientHost.register(blocked);
  try {
    const pending = c.connect();
    await within(blocked.entered.promise);
    ClientHost.unregister();
    ClientHost.register(app); // later calls go to the new registration
    blocked.resume.resolve();
    await within(pending);
    expect(await c.keyFingerprint()).toBe(app.fingerprint);
  } finally {
    blocked.resume.resolve();
    ClientHost.unregister();
  }
});

test("every instance can have its own keystore", async () => {
  // No process-wide registration here.
  const first = new AppKeys();
  const second = new AppKeys();
  using a = signedClient("app-key");
  using b = signedClient("app-key");
  using c = signedClient("app-key");
  ClientHost.registerFor(a, first);
  ClientHost.registerFor(b, second);
  await Promise.all([a.connect(), b.connect()]);
  expect(await a.keyFingerprint()).toBe(first.fingerprint);
  expect(await b.keyFingerprint()).toBe(second.fingerprint);
  // An instance without its own registration falls back to the process-wide one (here: none).
  expect((await errorOf(c.connect())).tag).toBe("key-error");
  // Unregistering one instance does not touch the other.
  ClientHost.unregisterFor(a);
  expect((await errorOf(a.getCustomer("4711"))).tag).toBe("key-error");
  expect((await b.getCustomer("4711")).id).toBe("4711");
});

describe("registered", () => {
  beforeAll(() => ClientHost.register(app));
  afterAll(() => ClientHost.unregister());

  test("the backend names the app key that signed the session", async () => {
    using c = signedClient("app-key");
    await c.connect();
    expect(await c.keyFingerprint()).toBe(app.fingerprint);
    const before = app.signatures;
    expect((await c.getCustomer("4711")).name).toBe("Erika Mustermann");
    expect(app.signatures - before).toBe(1); // one signature per request
  });

  test("concurrent signed calls", async () => {
    using c = signedClient("app-key");
    await c.connect();
    const before = app.signatures;
    const results = await Promise.all(
      Array.from({ length: 64 }, (_, i) => c.sendCommand({ name: "echo", payload: new TextEncoder().encode(`payload-${i}`) })),
    );
    expect(app.signatures - before).toBe(64);
    results.forEach((r, i) => expect(new TextDecoder().decode(r.payload)).toBe(`payload-${i}`)); // every signature ended up on its own request
  });

  test("app exception becomes the caller's ClientException", async () => {
    using c = signedClient("unknown-key");
    expect(await errorOf(c.connect())).toEqual({ tag: "key-error", value: "unknown-key" });
  });

  test("the app learns that the core gave up on a call", async () => {
    using c = signedClient("slow-key");
    await expect(c.connect(AbortSignal.timeout(100))).rejects.toBeInstanceOf(WabiCancelledException);
    await within(app.cancelled.promise);
  });

  test("a forged signature is rejected by the backend", async () => {
    using c = signedClient("forged-key");
    expect(await errorOf(c.connect())).toEqual({ tag: "unauthorized" });
  });
});
