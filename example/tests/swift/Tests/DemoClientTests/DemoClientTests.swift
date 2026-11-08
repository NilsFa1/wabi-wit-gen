// Same scenarios as tests/csharp and tests/java. Run with the core on the
// linker path: `swift test -Xlinker -L<dir with libdemo_client>` in tests/swift.
// The backend the core talks to is WABI_TEST_ENDPOINT (default: the in-process
// mock). Host calls: HostKeysTests.swift.
import Foundation
import XCTest
@testable import DemoClient

final class DemoClientTests: XCTestCase {
    static let endpoint = ProcessInfo.processInfo.environment["WABI_TEST_ENDPOINT"] ?? "mem://swift-tests"

    func options(key: String = "demo-key", endpoint: String = DemoClientTests.endpoint) -> ClientOptions {
        ClientOptions(endpoint: endpoint, apiKey: key, requestTimeoutMs: 10_000)
    }

    func connected() async throws -> DemoClient {
        let c = DemoClient(options: options())
        try await c.connect()
        return c
    }

    func testSyncCall() throws {
        XCTAssertEqual(try DemoClient(options: options()).state(), .disconnected)
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
        do { try await DemoClient(options: options(key: "wrong")).connect(); XCTFail() } catch ClientError.unauthorized {}
        do {
            try await DemoClient(options: options(endpoint: "http://127.0.0.1:1")).connect()
            XCTFail()
        } catch ClientError.networkError(let message) { XCTAssertFalse(message.isEmpty) }
        // A session without a key was not signed by anyone.
        do { _ = try await c.keyFingerprint(); XCTFail() } catch ClientError.unauthorized {}
    }

    func testCancellation() async throws {
        let c = try await connected()
        let slow = Task { try await c.sendCommand(command: Command(name: "sleep", payload: Array("5000".utf8))) }
        try await Task.sleep(nanoseconds: 50_000_000)
        let start = Date()
        slow.cancel()
        do { _ = try await slow.value; XCTFail() } catch is CancellationError {}
        XCTAssertLessThan(Date().timeIntervalSince(start), 2)
        let customer = try await c.getCustomer(id: "4711") // no `await` inside XCTAssert autoclosures
        XCTAssertEqual(customer.id, "4711")
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

    func testResourcesAsParametersAndResults() async throws {
        let transport = Transport(endpoint: Self.endpoint)
        let a = try transport.openClient(options: options(endpoint: "unused://")) // a resource as result
        let b = DemoClient(options: options())
        try b.useTransport(transport: transport) // lent for the call
        try await a.connect()
        try await b.connect()
        let before = try transport.requestCount()
        _ = try await a.getCustomer(id: "4711")
        _ = try await b.getCustomer(id: "4711")
        XCTAssertEqual(try transport.requestCount(), before + 2, "both clients share the transport")

        // An owned parameter moves into the core: the SDK object is gone afterwards.
        let other = Transport(endpoint: Self.endpoint)
        let c = try other.openClient(options: options())
        try await c.connect()
        let (mine, theirs) = (try transport.requestCount(), try other.requestCount())
        try transport.absorb(other: other)
        XCTAssertEqual(try transport.requestCount(), mine + theirs)
        XCTAssertThrowsError(try other.requestCount()) { XCTAssertEqual($0 as? WabiRuntimeError, .closed) }
        other.close() // harmless: nothing left to drop
    }

    func testObjectLifetimeAndDoubleDrop() async throws {
        let c = try await connected()
        c.close()
        c.close()
        XCTAssertThrowsError(try c.state()) { XCTAssertEqual($0 as? WabiRuntimeError, .closed) }
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
        let other = CoreInfo(abiVersion: info.abiVersion, capabilities: info.capabilities, contractId: "demo:client@2.0.0", coreDescription: "x")
        XCTAssertThrowsError(try other.ensureCompatible())
    }

    func testLargePayloadAndUnicode() async throws {
        let c = try await connected()
        let payload = (0..<(4 << 20)).map { UInt8(truncatingIfNeeded: $0 &* 31) }
        let echoed = try await c.sendCommand(command: Command(name: "echo", payload: payload))
        XCTAssertEqual(echoed.payload, payload)
        let name = "Jürgen Groß 张伟 🚀👩‍💻"
        let created = try await c.createCustomer(name: name, email: nil)
        XCTAssertEqual(created.name, name)
    }
}
