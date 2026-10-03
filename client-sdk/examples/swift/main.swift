// Plain application code against the generated Swift SDK. Which core runs
// underneath (Rust, Go, Zig) is decided by the linked library — not here.
import WabiClient

let options = ClientOptions(endpoint: CommandLine.arguments.dropFirst().first ?? "mem://example",
                            apiKey: "demo-key", requestTimeoutMs: 5_000)
let client = WabiClient(options: options)

let listener = Task {
    for try await event in client.events() {
        print("  event: \(event)")
        if case .messageReceived = event { break }
    }
}

try await client.connect()
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
