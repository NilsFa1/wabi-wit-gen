// The app side of host calls in TypeScript (Bun): the app owns the key (here
// @noble/curves standing in for a KMS/HSM; same SPKI and DER formats), the core
// builds the DPoP proof and asks the app to sign. The core under test is chosen
// with WABI_NATIVE_LIB. Same scenarios as the C#, Java, Dart and Swift tests.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { p256 } from "@noble/curves/p256";
import { DpopSigner, NativeBackend, ZeroException, ZeroHost, type HostKeys } from "../src/index.ts";

/** SubjectPublicKeyInfo header for id-ecPublicKey / prime256v1 (+ uncompressed point). */
const SPKI_PREFIX = Buffer.from("3059301306072a8648ce3d020106082a8648ce3d030107034200", "hex");

const sha256 = (data: string | Uint8Array) => createHash("sha256").update(data).digest();
const b64url = (b: Uint8Array) => Buffer.from(b).toString("base64url");
const unb64url = (s: string) => Buffer.from(s, "base64url");
const json = (s: string) => JSON.parse(unb64url(s).toString("utf8"));

/** The "keystore" of the app. Platform keystores return SPKI DER public keys and ASN.1 DER signatures. */
class AppKeys implements HostKeys {
  readonly priv = p256.utils.randomPrivateKey();
  readonly point = p256.getPublicKey(this.priv, false); // 0x04 || X || Y
  signatures = 0;

  get x() { return this.point.subarray(1, 33); }
  get y() { return this.point.subarray(33, 65); }

  thumbprint(): string {
    return b64url(sha256(`{"crv":"P-256","kty":"EC","x":"${b64url(this.x)}","y":"${b64url(this.y)}"}`));
  }

  publicKey(keyId: string): Uint8Array {
    if (keyId !== "dpop-key") throw new ZeroException({ tag: "key-not-found", value: keyId });
    return Buffer.concat([SPKI_PREFIX, this.point]);
  }

  async sign(keyId: string, digest: Uint8Array): Promise<Uint8Array> {
    if (keyId !== "dpop-key") throw new ZeroException({ tag: "key-not-found", value: keyId });
    await Bun.sleep(Math.floor(Math.random() * 6)); // e.g. a biometric prompt: answering later (and out of order) is fine
    this.signatures++;
    return p256.sign(digest, this.priv, { prehash: false }).toDERRawBytes();
  }

  verifyJws(jwt: string): boolean {
    const [h, c, s] = jwt.split(".");
    const rs = unb64url(s!);
    return rs.length === 64 && p256.verify(rs, sha256(`${h}.${c}`), this.point, { prehash: false });
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
const within = <T>(p: Promise<T>, ms = 5000) =>
  Promise.race([p, Bun.sleep(ms).then(() => { throw new Error(`timeout after ${ms} ms`); })]);

beforeAll(() => console.log(`core under test: ${NativeBackend.shared.description}`));

test("without registration the core reports a domain error", async () => {
  using signer = new DpopSigner("dpop-key");
  const err = await signer.thumbprint().catch((e) => e);
  expect(err).toBeInstanceOf(ZeroException);
  expect((err as ZeroException).error.tag).toBe("crypto");
});

test("an in-flight host call survives unregister and replacement", async () => {
  using signer = new DpopSigner("dpop-key");
  const blocked = new BlockingKeys(app);
  ZeroHost.register(blocked);
  try {
    const pending = signer.thumbprint();
    await within(blocked.entered.promise);
    ZeroHost.unregister();
    ZeroHost.register(new AppKeys()); // future calls use a new key
    blocked.resume.resolve();
    expect(await within(pending)).toBe(app.thumbprint());
  } finally {
    blocked.resume.resolve();
    ZeroHost.unregister();
  }
});

describe("registered", () => {
  beforeAll(() => ZeroHost.register(app));
  afterAll(() => ZeroHost.unregister());

  test("thumbprint (core) equals RFC 7638 computed here", async () => {
    using signer = new DpopSigner("dpop-key");
    expect(await signer.thumbprint()).toBe(app.thumbprint());
  });

  test("DPoP proof is signed by the app key", async () => {
    using signer = new DpopSigner("dpop-key");
    const jwt = await signer.createProof("GET", "https://zero.example/api", "at-1", "n-1");
    expect(app.verifyJws(jwt)).toBe(true);
    const [h, c] = jwt.split(".");
    const header = json(h!);
    const claims = json(c!);
    expect(header.typ).toBe("dpop+jwt");
    expect(header.jwk.x).toBe(b64url(app.x));
    expect(claims.htm).toBe("GET");
    expect(claims.htu).toBe("https://zero.example/api");
    expect(claims.ath).toBe(b64url(sha256("at-1")));
    expect(claims.nonce).toBe("n-1");
  });

  test("concurrent proofs", async () => {
    using signer = new DpopSigner("dpop-key");
    const before = app.signatures;
    const proofs = await Promise.all(
      Array.from({ length: 64 }, (_, i) => signer.createProof("POST", `https://zero.example/${i}`, undefined, undefined)),
    );
    expect(new Set(proofs).size).toBe(64);
    expect(app.signatures - before).toBe(64);
    proofs.forEach((jwt, i) => { // every answer belongs to its own request
      expect(app.verifyJws(jwt)).toBe(true);
      expect(json(jwt.split(".")[1]!).htu).toBe(`https://zero.example/${i}`);
    });
  });

  test("app exception becomes the caller's ZeroError", async () => {
    using signer = new DpopSigner("unknown-key");
    const err = await signer.createProof("GET", "https://zero.example/", undefined, undefined).catch((e) => e);
    expect(err).toBeInstanceOf(ZeroException);
    expect((err as ZeroException).error).toEqual({ tag: "key-not-found", value: "unknown-key" });
  });

  test("invalid input is rejected by the core", async () => {
    using signer = new DpopSigner("dpop-key");
    const err = await signer.createProof("", "ftp://x", undefined, undefined).catch((e) => e);
    expect((err as ZeroException).error.tag).toBe("invalid-input");
  });
});
