// Plain application code against the generated TypeScript SDK (Bun). Which core
// runs underneath (Rust, Go) is decided by the native library — not here.
//   (cd apps/ts && bun install) && WABI_NATIVE_LIB=<path to the core library> bun apps/ts/main.ts [endpoint]
import { p256 } from "@noble/curves/p256";
import { ClientException, ClientHost, DemoClient, type HostKeys } from "../../sdk/ts/index.ts";

/**
 * The app's keystore: an EC P-256 key that never leaves the app (here in
 * memory; a real app would use a KMS/HSM and answer in the same formats).
 * The core asks for the public key and for signatures through host calls.
 */
class AppKeys implements HostKeys {
  private readonly key = p256.utils.randomPrivateKey();

  /** SubjectPublicKeyInfo (DER): header for id-ecPublicKey / prime256v1 + 0x04 || X || Y. */
  publicKey(): Uint8Array {
    const header = Buffer.from("3059301306072a8648ce3d020106082a8648ce3d030107034200", "hex");
    return Buffer.concat([header, p256.getPublicKey(this.key, false)]);
  }

  /** ECDSA signature (ASN.1 DER) over the SHA-256 digest the core computed. */
  sign(_keyId: string, digest: Uint8Array): Uint8Array {
    return p256.sign(digest, this.key, { prehash: false }).toDERRawBytes();
  }
}

using client = new DemoClient({ endpoint: process.argv[2] ?? "mem://example", apiKey: "demo-key", requestTimeoutMs: 5_000, keyId: "app-key" });
// The app keeps the signing key; the core asks for every signature (host calls).
ClientHost.registerFor(client, new AppKeys());

const listener = (async () => {
  for await (const event of client.events()) {
    console.log("  event:", event);
    if (event.tag === "message-received") break; // releases the native stream
  }
})();

await client.connect();
console.log("backend verified our key:", await client.keyFingerprint());
const customer = await client.getCustomer("4711");
console.log(`customer: ${customer.name} <${customer.email ?? "-"}>`);

try {
  await client.getCustomer("0000");
} catch (e) {
  if (e instanceof ClientException && e.error.tag === "not-found") console.log("customer 0000: not found");
  else throw e;
}

console.log("created:", await client.createCustomer("Max Mustermann", undefined));
await client.sendCommand({ name: "broadcast", payload: new TextEncoder().encode("hello from TypeScript") });

await listener;
await client.disconnect();
console.log("state:", client.state());
