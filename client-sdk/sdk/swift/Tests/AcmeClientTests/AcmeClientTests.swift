// Same scenarios as tests/csharp and tests/java. Run with the core on the
// linker path, e.g. `swift test -Xlinker -L../../core-rust/target/release`.
import CAcmeClient
import Foundation
import XCTest
@testable import AcmeClient

final class AcmeClientTests: XCTestCase {
    func options(key: String = "demo-key", endpoint: String = "mem://swift-tests") -> ClientOptions {
        ClientOptions(endpoint: endpoint, apiKey: key, requestTimeoutMs: 10_000)
    }

    func connected() async throws -> AcmeClient {
        let c = AcmeClient(options: options())
        try await c.connect()
        return c
    }

    func testSyncCall() throws {
        XCTAssertEqual(try AcmeClient(options: options()).state(), .disconnected)
    }

    func testAsyncCall() async throws {
        let c = try await connected()
        let customer = try await c.getCustomer(id: "4711")
        XCTAssertEqual(customer, Customer(id: "4711", name: "Erika Mustermann", email: "erika@example.com"))
        try await c.disconnect()
        XCTAssertEqual(try c.state(), .disconnected)
    }

    func testAsyncError() async throws {
        let c = try await connected()
        do { _ = try await c.getCustomer(id: "nope"); XCTFail() } catch ClientError.notFound {}
        do { try await AcmeClient(options: options(key: "wrong")).connect(); XCTFail() } catch ClientError.unauthorized {}
        do {
            try await AcmeClient(options: options(endpoint: "mem://unreachable")).connect()
            XCTFail()
        } catch ClientError.networkError(let message) { XCTAssertFalse(message.isEmpty) }
    }

    func testCancellation() async throws {
        let c = try await connected()
        let slow = Task { try await c.sendCommand(command: Command(name: "sleep", payload: Array("5000".utf8))) }
        try await Task.sleep(nanoseconds: 50_000_000)
        let start = Date()
        slow.cancel()
        do { _ = try await slow.value; XCTFail() } catch is CancellationError {}
        XCTAssertLessThan(Date().timeIntervalSince(start), 2)
        XCTAssertEqual(try await c.getCustomer(id: "4711").id, "4711")
    }

    func testStreamAndStreamCancellation() async throws {
        let c = try await connected()
        let reader = Task { () -> [ClientEvent] in
            var events: [ClientEvent] = []
            for try await e in c.events() {
                events.append(e)
                if events.count == 2 { break }
            }
            return events
        }
        try await Task.sleep(nanoseconds: 100_000_000)
        _ = try await c.sendCommand(command: Command(name: "broadcast", payload: Array("hello 👋".utf8)))
        let created = try await c.createCustomer(name: "Max", email: nil)
        let events = try await reader.value
        XCTAssertEqual(events, [.messageReceived("hello 👋"), .customerUpdated(created)])

        let endless = Task { for try await _ in c.events() {} }
        try await Task.sleep(nanoseconds: 50_000_000)
        endless.cancel()
        do { try await endless.value; XCTFail() } catch is CancellationError {}
    }

    func testObjectLifetimeAndDoubleDrop() async throws {
        let c = try await connected()
        c.close()
        c.close()
        XCTAssertThrowsError(try c.state()) { XCTAssertEqual($0 as? AcmeRuntimeError, .closed) }
        XCTAssertEqual(acme_object_drop(0xdead), AcmeAbi.errInvalidHandle)
    }

    func testConcurrentCalls() async throws {
        let c = try await connected()
        try await withThrowingTaskGroup(of: Customer.self) { group in
            for _ in 0..<500 { group.addTask { try await c.getCustomer(id: "4711") } }
            for try await customer in group { XCTAssertEqual(customer.name, "Erika Mustermann") }
        }
    }

    func testCallbackDuringDispose() async throws {
        let c = try await connected()
        let slow = Task { try await c.sendCommand(command: Command(name: "sleep", payload: Array("3000".utf8))) }
        try await Task.sleep(nanoseconds: 100_000_000)
        c.close()
        do { _ = try await slow.value; XCTFail() } catch ClientError.disconnected {} catch is CancellationError {}
    }

    func testVersionMismatch() throws {
        let info = try NativeBackend.shared.get().info
        XCTAssertNoThrow(try info.ensureCompatible())
        let v2 = CoreInfo(abiVersion: 2 << 16, capabilities: info.capabilities, contractId: info.contractId, coreDescription: "v2")
        XCTAssertThrowsError(try v2.ensureCompatible())
        let other = CoreInfo(abiVersion: info.abiVersion, capabilities: info.capabilities, contractId: "acme:client@2.0.0", coreDescription: "x")
        XCTAssertThrowsError(try other.ensureCompatible())
    }

    func testLargePayloadAndUnicode() async throws {
        let c = try await connected()
        let payload = (0..<(4 << 20)).map { UInt8(truncatingIfNeeded: $0 &* 31) }
        XCTAssertEqual(try await c.sendCommand(command: Command(name: "echo", payload: payload)).payload, payload)
        let name = "Jürgen Groß 张伟 🚀👩‍💻"
        let created = try await c.createCustomer(name: name, email: nil)
        XCTAssertEqual(created.name, name)
    }
}
