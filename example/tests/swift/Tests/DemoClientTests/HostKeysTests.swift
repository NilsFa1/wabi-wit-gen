// The app side of host calls in Swift: the app owns the key in the Security
// framework (the same API that keeps keys in the Secure Enclave on iOS); the
// core asks it to sign every request. The backend verifies the signatures.
// Same scenarios as tests/csharp, tests/java and the Dart tests.
// Run on macOS: `swift test -Xlinker -L<dir with libdemo_client.dylib>`.
#if canImport(Security)
import CryptoKit
import Foundation
import Security
import XCTest
@testable import DemoClient

/// SubjectPublicKeyInfo header for id-ecPublicKey / prime256v1 (+ uncompressed point).
private let spkiPrefix: [UInt8] = [0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01,
                                   0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00]

private func newKey() -> SecKey {
    let attributes: [String: Any] = [kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
                                     kSecAttrKeySizeInBits as String: 256]
    return SecKeyCreateRandomKey(attributes as CFDictionary, nil)!
}

final class AppKeys: HostKeys, @unchecked Sendable {
    let key = newKey()
    let other = newKey()
    private let lock = NSLock()
    private var count = 0
    var signatures: Int { lock.withLock { count } }
    private var cancelledFlag = false
    /// True once the core gave up on a call for "slow-key".
    var cancelled: Bool { lock.withLock { cancelledFlag } }

    /// SubjectPublicKeyInfo: header + 0x04 || X || Y
    var spki: [UInt8] { spkiPrefix + [UInt8](SecKeyCopyExternalRepresentation(SecKeyCopyPublicKey(key)!, nil)! as Data) }

    var fingerprint: String { SHA256.hash(data: Data(spki)).map { String(format: "%02x", $0) }.joined() }

    /// "forged-key" announces the app's key but signs with another one.
    private func known(_ keyId: String) -> Bool { keyId == "app-key" || keyId == "forged-key" }

    func publicKey(keyId: String) async throws -> [UInt8] {
        if keyId == "slow-key" { // e.g. waiting for the user: ends when the core no longer waits
            do {
                try await Task.sleep(nanoseconds: 30_000_000_000)
            } catch {
                lock.withLock { cancelledFlag = true }
                throw error
            }
        }
        guard known(keyId) else { throw ClientError.keyError(keyId) }
        return spki
    }

    func sign(keyId: String, digest: [UInt8]) async throws -> [UInt8] {
        guard known(keyId) else { throw ClientError.keyError(keyId) }
        var error: Unmanaged<CFError>?
        let signer = keyId == "app-key" ? key : other
        guard let sig = SecKeyCreateSignature(signer, .ecdsaSignatureDigestX962SHA256, Data(digest) as CFData, &error) else {
            throw ClientError.keyError("SecKeyCreateSignature failed")
        }
        lock.withLock { count += 1 }
        return [UInt8](sig as Data) // ASN.1 DER, as keystores return it
    }
}

final class HostKeysTests: XCTestCase {
    static let app = AppKeys()

    func signedClient(_ keyId: String) -> DemoClient {
        DemoClient(options: ClientOptions(endpoint: DemoClientTests.endpoint, apiKey: "demo-key", requestTimeoutMs: 10_000, keyId: keyId))
    }

    func testA_withoutRegistrationTheCoreReportsADomainError() async throws {
        ClientHost.unregister()
        do {
            try await signedClient("app-key").connect()
            XCTFail("expected ClientError")
        } catch ClientError.keyError {
            // expected
        }
    }

    func testB_theBackendNamesTheAppKeyThatSignedTheSession() async throws {
        try ClientHost.register(hostKeys: Self.app)
        defer { ClientHost.unregister() }
        let c = signedClient("app-key")
        try await c.connect()
        let fingerprint = try await c.keyFingerprint()
        XCTAssertEqual(fingerprint, Self.app.fingerprint)
        let before = Self.app.signatures
        let customer = try await c.getCustomer(id: "4711")
        XCTAssertEqual(customer.name, "Erika Mustermann")
        XCTAssertEqual(Self.app.signatures - before, 1, "one signature per request")
    }

    func testC_concurrentSignedCalls() async throws {
        try ClientHost.register(hostKeys: Self.app)
        defer { ClientHost.unregister() }
        let c = signedClient("app-key")
        try await c.connect()
        let before = Self.app.signatures
        try await withThrowingTaskGroup(of: (Int, CommandResult).self) { group in
            for i in 0..<64 {
                group.addTask { (i, try await c.sendCommand(command: Command(name: "echo", payload: Array("payload-\(i)".utf8)))) }
            }
            for try await (i, result) in group { // every signature ended up on its own request
                XCTAssertEqual(String(decoding: result.payload, as: UTF8.self), "payload-\(i)")
            }
        }
        XCTAssertEqual(Self.app.signatures - before, 64)
    }

    func testD_appErrorBecomesTheCallersClientError() async throws {
        try ClientHost.register(hostKeys: Self.app)
        defer { ClientHost.unregister() }
        do {
            try await signedClient("unknown-key").connect()
            XCTFail("expected ClientError")
        } catch let e as ClientError {
            XCTAssertEqual(e, .keyError("unknown-key"))
        }
    }

    func testF_everyInstanceCanHaveItsOwnKeystore() async throws {
        ClientHost.unregister() // no process-wide registration here
        let first = AppKeys(), second = AppKeys()
        let a = signedClient("app-key"), b = signedClient("app-key"), c = signedClient("app-key")
        try ClientHost.register(for: a, hostKeys: first)
        try ClientHost.register(for: b, hostKeys: second)
        try await a.connect()
        try await b.connect()
        let (fa, fb) = (try await a.keyFingerprint(), try await b.keyFingerprint())
        XCTAssertEqual(fa, first.fingerprint)
        XCTAssertEqual(fb, second.fingerprint)
        // An instance without its own registration falls back to the process-wide one (here: none).
        do { try await c.connect(); XCTFail("expected ClientError") } catch ClientError.keyError {}
        // Unregistering one instance does not touch the other.
        ClientHost.unregister(for: a)
        do { _ = try await a.getCustomer(id: "4711"); XCTFail("expected ClientError") } catch ClientError.keyError {}
        let customer = try await b.getCustomer(id: "4711")
        XCTAssertEqual(customer.id, "4711")
        a.close(); b.close(); c.close()
    }

    func testG_theAppLearnsThatTheCoreGaveUpOnACall() async throws {
        let app = AppKeys()
        try ClientHost.register(hostKeys: app)
        defer { ClientHost.unregister() }
        let c = signedClient("slow-key")
        let connecting = Task { try await c.connect() }
        try await Task.sleep(nanoseconds: 100_000_000)
        connecting.cancel()
        do { try await connecting.value; XCTFail("expected cancellation") } catch is CancellationError {}
        for _ in 0..<500 where !app.cancelled { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertTrue(app.cancelled, "the task running the app's publicKey was cancelled")
    }

    func testE_aForgedSignatureIsRejectedByTheBackend() async throws {
        try ClientHost.register(hostKeys: Self.app)
        defer { ClientHost.unregister() }
        do {
            try await signedClient("forged-key").connect()
            XCTFail("expected ClientError")
        } catch ClientError.unauthorized {
            // expected
        }
    }
}
#endif
