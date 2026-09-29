// The Swift app side of gematik:pki: TUC_PKI_018 certificate verification with
// gemLibPki (Java) running as a GraalVM native library. Same scenarios as the
// C#, Java and Dart tests; fixtures from PKI_FIXTURES (see packages/pki/tests/fixtures).
import Foundation
import XCTest
@testable import GematikPki

final class GematikPkiTests: XCTestCase {
    static let fixtures = ProcessInfo.processInfo.environment["PKI_FIXTURES"] ?? "../../tests/fixtures"

    func load(_ name: String) throws -> [UInt8] {
        [UInt8](try Data(contentsOf: URL(fileURLWithPath: "\(Self.fixtures)/\(name)")))
    }

    func verifier() throws -> CertificateVerifier {
        CertificateVerifier(productType: "PoC-Test", tsl: try load("tsl.xml"))
    }

    func code(_ body: () async throws -> Admission) async -> String {
        do {
            _ = try await body()
            return "no error"
        } catch PkiError.verificationFailed(let f) {
            return f.code
        } catch {
            return "\(error)"
        }
    }

    func testValidCertificate() async throws {
        let a = try await verifier().verify(certificate: load("valid.der"), profiles: [.cHciAutEcc], withOcsp: false)
        XCTAssertEqual(a.profile, .cHciAutEcc)
        XCTAssertEqual(a.registrationNumber, "1-2-ARZT-Test-01")
        XCTAssertEqual(a.professionItems, ["Betriebsstätte Arzt"])
    }

    func testFailures() async throws {
        let v = try verifier()
        let expired = try load("expired.der"), wrongKu = try load("wrong-key-usage.der"), unknown = try load("unknown-ca.der")
        let r1 = await code { try await v.verify(certificate: expired, profiles: [.cHciAutEcc], withOcsp: false) }
        XCTAssertEqual(r1, "SE_1021_CERTIFICATE_NOT_VALID_TIME")
        let r2 = await code { try await v.verify(certificate: wrongKu, profiles: [.cHciAutEcc], withOcsp: false) }
        XCTAssertEqual(r2, "SE_1016_WRONG_KEYUSAGE")
        let r3 = await code { try await v.verify(certificate: unknown, profiles: [.cHciAutEcc], withOcsp: false) }
        XCTAssertEqual(r3, "TE_1027_CA_CERT_MISSING")
    }
}
