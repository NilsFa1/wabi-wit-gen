// The app side of host calls in Swift: the app owns the key in the Security
// framework (the same API that keeps keys in the Secure Enclave on iOS), the
// Go core (libzero) builds the DPoP proof and asks the app to sign.
// Same scenarios as tests/csharp, tests/java and the Dart tests.
// Run on macOS: `swift test -Xlinker -L<dir with libgematik_zero.dylib>`.
#if canImport(Security)
import Foundation
import Security
import XCTest
@testable import GematikZero

/// SubjectPublicKeyInfo header for id-ecPublicKey / prime256v1 (+ uncompressed point).
private let spkiPrefix: [UInt8] = [0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01,
                                   0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00]

final class AppKeys: HostKeys, @unchecked Sendable {
    let key: SecKey
    private let lock = NSLock()
    private var count = 0
    var signatures: Int { lock.lock(); defer { lock.unlock() }; return count }

    init() {
        let attributes: [String: Any] = [kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
                                         kSecAttrKeySizeInBits as String: 256]
        key = SecKeyCreateRandomKey(attributes as CFDictionary, nil)!
    }

    /// 0x04 || X || Y
    var point: [UInt8] { [UInt8](SecKeyCopyExternalRepresentation(SecKeyCopyPublicKey(key)!, nil)! as Data) }

    func publicKey(keyId: String) async throws -> [UInt8] {
        guard keyId == "dpop-key" else { throw ZeroError.keyNotFound(keyId) }
        return spkiPrefix + point
    }

    func sign(keyId: String, digest: [UInt8]) async throws -> [UInt8] {
        guard keyId == "dpop-key" else { throw ZeroError.keyNotFound(keyId) }
        var error: Unmanaged<CFError>?
        guard let sig = SecKeyCreateSignature(key, .ecdsaSignatureDigestX962SHA256, Data(digest) as CFData, &error) else {
            throw ZeroError.crypto("SecKeyCreateSignature failed")
        }
        lock.lock(); count += 1; lock.unlock()
        return [UInt8](sig as Data) // ASN.1 DER, as keystores return it
    }

    /// Verifies an ES256 JWS (r||s) with the app's public key.
    func verify(_ jwt: String) -> Bool {
        let parts = jwt.split(separator: ".").map(String.init)
        guard parts.count == 3, let rs = unb64url(parts[2]), rs.count == 64 else { return false }
        func derInt(_ b: ArraySlice<UInt8>) -> [UInt8] {
            var v = Array(b.drop(while: { $0 == 0 }))
            if v.isEmpty || v[0] & 0x80 != 0 { v.insert(0, at: 0) }
            return [0x02, UInt8(v.count)] + v
        }
        let body = derInt(rs[0..<32]) + derInt(rs[32..<64])
        let der = Data([0x30, UInt8(body.count)] + body)
        return SecKeyVerifySignature(SecKeyCopyPublicKey(key)!, .ecdsaSignatureMessageX962SHA256,
                                     Data("\(parts[0]).\(parts[1])".utf8) as CFData, der as CFData, nil)
    }
}

func b64url(_ b: [UInt8]) -> String {
    Data(b).base64EncodedString().replacingOccurrences(of: "+", with: "-")
        .replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
}

func unb64url(_ s: String) -> [UInt8]? {
    var t = s.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    while t.count % 4 != 0 { t += "=" }
    return Data(base64Encoded: t).map { [UInt8]($0) }
}

func json(_ part: String) -> [String: Any] {
    (try? JSONSerialization.jsonObject(with: Data(unb64url(part) ?? []))) as? [String: Any] ?? [:]
}

final class GematikZeroTests: XCTestCase {
    static let app = AppKeys()

    func testA_withoutRegistrationTheCoreReportsADomainError() async throws {
        ZeroHost.unregister()
        let signer = DpopSigner(keyId: "dpop-key")
        do {
            _ = try await signer.thumbprint()
            XCTFail("expected ZeroError")
        } catch ZeroError.crypto {
            // expected
        }
    }

    func testB_proofIsSignedByTheAppKey() async throws {
        try ZeroHost.register(hostKeys: Self.app)
        defer { ZeroHost.unregister() }
        let signer = DpopSigner(keyId: "dpop-key")
        let jwt = try await signer.createProof(httpMethod: "GET", httpUrl: "https://zero.example/api", accessToken: "at-1", nonce: nil)
        XCTAssertTrue(Self.app.verify(jwt))
        let parts = jwt.split(separator: ".").map(String.init)
        XCTAssertEqual(json(parts[0])["typ"] as? String, "dpop+jwt")
        XCTAssertEqual(json(parts[1])["htm"] as? String, "GET")
        XCTAssertEqual(json(parts[1])["htu"] as? String, "https://zero.example/api")
        XCTAssertNil(json(parts[1])["nonce"])
    }

    func testC_thumbprintMatchesTheAppKey() async throws {
        try ZeroHost.register(hostKeys: Self.app)
        defer { ZeroHost.unregister() }
        let p = Self.app.point
        let jwk = "{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"\(b64url(Array(p[1..<33])))\",\"y\":\"\(b64url(Array(p[33..<65])))\"}"
        let tp = try await DpopSigner(keyId: "dpop-key").thumbprint()
        XCTAssertEqual(tp, b64url(sha256(Array(jwk.utf8))))
    }

    func testD_concurrentProofs() async throws {
        try ZeroHost.register(hostKeys: Self.app)
        defer { ZeroHost.unregister() }
        let signer = DpopSigner(keyId: "dpop-key")
        let before = Self.app.signatures
        let proofs = try await withThrowingTaskGroup(of: String.self) { group in
            for i in 0..<16 {
                group.addTask { try await signer.createProof(httpMethod: "POST", httpUrl: "https://zero.example/\(i)", accessToken: nil, nonce: "n") }
            }
            return try await group.reduce(into: [String]()) { $0.append($1) }
        }
        XCTAssertEqual(Set(proofs).count, 16)
        XCTAssertEqual(Self.app.signatures - before, 16)
        XCTAssertTrue(proofs.allSatisfy(Self.app.verify))
    }

    func testE_appErrorBecomesTheCallersZeroError() async throws {
        try ZeroHost.register(hostKeys: Self.app)
        defer { ZeroHost.unregister() }
        do {
            _ = try await DpopSigner(keyId: "unknown-key").createProof(httpMethod: "GET", httpUrl: "https://zero.example/", accessToken: nil, nonce: nil)
            XCTFail("expected ZeroError")
        } catch let e as ZeroError {
            XCTAssertEqual(e, .keyNotFound("unknown-key"))
        }
    }
}

/// Minimal SHA-256 (FIPS 180-4) so the test needs no CryptoKit.
func sha256(_ message: [UInt8]) -> [UInt8] {
    let k: [UInt32] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ]
    var h: [UInt32] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]
    var msg = message + [0x80]
    while msg.count % 64 != 56 { msg.append(0) }
    let bits = UInt64(message.count) * 8
    for i in (0..<8).reversed() { msg.append(UInt8(truncatingIfNeeded: bits >> (UInt64(i) * 8))) }
    func rotr(_ x: UInt32, _ n: UInt32) -> UInt32 { (x >> n) | (x << (32 - n)) }
    for chunk in stride(from: 0, to: msg.count, by: 64) {
        var w = [UInt32](repeating: 0, count: 64)
        for i in 0..<16 {
            w[i] = (0..<4).reduce(0) { $0 << 8 | UInt32(msg[chunk + i * 4 + $1]) }
        }
        for i in 16..<64 {
            let s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3)
            let s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10)
            w[i] = w[i - 16] &+ s0 &+ w[i - 7] &+ s1
        }
        var (a, b, c, d, e, f, g, hh) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7])
        for i in 0..<64 {
            let s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)
            let t1 = hh &+ s1 &+ ((e & f) ^ (~e & g)) &+ k[i] &+ w[i]
            let s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)
            let t2 = s0 &+ ((a & b) ^ (a & c) ^ (b & c))
            (hh, g, f, e, d, c, b, a) = (g, f, e, d &+ t1, c, b, a, t1 &+ t2)
        }
        for (i, v) in [a, b, c, d, e, f, g, hh].enumerated() { h[i] = h[i] &+ v }
    }
    return h.flatMap { v in (0..<4).map { UInt8(truncatingIfNeeded: v >> (24 - 8 * UInt32($0))) } }
}
#endif
