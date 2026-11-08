// Plain application code against the generated Swift SDK. Which core runs
// underneath (Rust, Go) is decided by the linked library — not here.
import Foundation
import DemoClient

#if canImport(Security)
import Security

/// The app's keystore: an EC P-256 key in the Security framework (the API that
/// keeps keys in the Secure Enclave on iOS). The private key never leaves it;
/// the core asks for the public key and for signatures through host calls.
final class AppKeys: HostKeys, @unchecked Sendable {
    private let key = SecKeyCreateRandomKey([kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
                                             kSecAttrKeySizeInBits as String: 256] as CFDictionary, nil)!

    /// SubjectPublicKeyInfo (DER): header for id-ecPublicKey / prime256v1 + 0x04 || X || Y.
    func publicKey(keyId: String) async throws -> [UInt8] {
        let header: [UInt8] = [0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01,
                               0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00]
        return header + [UInt8](SecKeyCopyExternalRepresentation(SecKeyCopyPublicKey(key)!, nil)! as Data)
    }

    /// ECDSA signature (ASN.1 DER) over the SHA-256 digest the core computed.
    func sign(keyId: String, digest: [UInt8]) async throws -> [UInt8] {
        guard let signature = SecKeyCreateSignature(key, .ecdsaSignatureDigestX962SHA256, Data(digest) as CFData, nil) else {
            throw ClientError.keyError("SecKeyCreateSignature failed")
        }
        return [UInt8](signature as Data)
    }
}
let keyId: String? = "app-key"
#else
let keyId: String? = nil // no keystore on this platform: requests are not signed
#endif

let options = ClientOptions(endpoint: CommandLine.arguments.dropFirst().first ?? "mem://example",
                            apiKey: "demo-key", requestTimeoutMs: 5_000, keyId: keyId)
let client = DemoClient(options: options)
#if canImport(Security)
// The app keeps the signing key; the core asks for every signature (host calls).
try ClientHost.register(for: client, hostKeys: AppKeys())
#endif

let listener = Task {
    for try await event in client.events() {
        print("  event: \(event)")
        if case .messageReceived = event { break }
    }
}

try await client.connect()
if keyId != nil { print("backend verified our key: \(try await client.keyFingerprint())") }
let customer = try await client.getCustomer(id: "4711")
print("customer: \(customer.name) <\(customer.email ?? "-")>")

do {
    _ = try await client.getCustomer(id: "0000")
} catch ClientError.notFound {
    print("customer 0000: not found")
}

let created = try await client.createCustomer(name: "Max Mustermann", email: nil)
print("created: \(created)")
_ = try await client.sendCommand(command: Command(name: "broadcast", payload: Array("hello from Swift".utf8)))

try await listener.value
try await client.disconnect()
print("state: \(try client.state())")
client.close()
