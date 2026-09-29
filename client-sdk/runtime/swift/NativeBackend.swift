// Hand-written, generic: executes contract calls through the Acme native ABI
// (C module `CAcmeClient`: abi/acme_abi.h + generated/c/acme_client.h). No per-method code here.

import CAcmeClient

/// Identity and version of the linked core, plus the compatibility rule.
public struct CoreInfo: Sendable, CustomStringConvertible {
    public let abiVersion: UInt32
    public let capabilities: UInt64
    public let contractId: String
    public let coreDescription: String

    public var abiMajor: UInt32 { abiVersion >> 16 }
    public var abiMinor: UInt32 { abiVersion & 0xffff }
    public var description: String { "\(coreDescription) (abi \(abiMajor).\(abiMinor), \(contractId))" }

    /// Required capabilities; cancellation / threaded-callback are informative (cancel is always callable).
    public static let required: UInt64 =
        AcmeAbi.capSyncCalls | AcmeAbi.capAsyncCalls | AcmeAbi.capStreams | AcmeAbi.capPayloadMsgpack1

    /// ABI: same major, core minor >= SDK minor. Contract: same package + major, core minor >= SDK minor.
    public func ensureCompatible() throws {
        guard abiMajor == AcmeAbi.major, abiMinor >= AcmeAbi.minor else {
            throw AcmeRuntimeError.incompatibleLibrary("core '\(coreDescription)' implements ABI \(abiMajor).\(abiMinor), SDK needs \(AcmeAbi.major).\(AcmeAbi.minor)+")
        }
        let needed = CoreInfo.required | (AcmeAbi.requiresHostCalls ? AcmeAbi.capHostCalls : 0)
        guard capabilities & needed == needed else {
            throw AcmeRuntimeError.incompatibleLibrary("core '\(coreDescription)' lacks required capabilities")
        }
        let parts = contractId.split(separator: "@", maxSplits: 1)
        let version = parts.count == 2 ? parts[1].split(separator: ".").compactMap { Int($0) } : []
        guard parts.first.map(String.init) == Contract.package, version.count >= 2,
              version[0] == Contract.major, version[1] >= Contract.minor else {
            throw AcmeRuntimeError.incompatibleLibrary("core implements '\(contractId)', SDK was generated for '\(Contract.id)'")
        }
    }
}

/// Holds the continuation of one subscribed future until the core's callback.
private final class ReadyBox {
    let continuation: CheckedContinuation<Int32, Never>
    init(_ c: CheckedContinuation<Int32, Never>) { continuation = c }
}

/// Invoked by the core on one of ITS threads, exactly once per successful
/// subscribe. `resume` only enqueues the awaiting task on its executor, so
/// this returns immediately. Main-thread (UI) dispatch is the app's business
/// (`@MainActor` code hops back automatically after `await`).
private let onReady: acme_future_callback = { userData, _ in
    guard let userData else { return }
    Unmanaged<ReadyBox>.fromOpaque(userData).takeRetainedValue().continuation.resume(returning: AcmeAbi.ok)
}

public final class NativeBackend: ClientBackend, @unchecked Sendable {
    /// The process-wide backend for the linked core, validated on first use.
    public static let shared: Result<NativeBackend, Error> = Result {
        let backend = NativeBackend()
        try backend.info.ensureCompatible()
        return backend
    }

    public let info: CoreInfo
    public var description: String { "native: \(info)" }

    private init() {
        info = CoreInfo(
            abiVersion: acme_abi_version(),
            capabilities: acme_abi_capabilities(),
            contractId: String(cString: acme_contract_id()),
            coreDescription: String(cString: acme_core_info()))
    }

    // ------------------------------------------------------------ buffers

    /// Copies a core-owned buffer into Swift memory and frees it (always).
    private static func consume(_ buffer: acme_buffer) -> [UInt8] {
        defer { acme_buffer_free(buffer) }
        guard let ptr = buffer.ptr, buffer.len > 0 else { return [] }
        return Array(UnsafeBufferPointer(start: ptr, count: Int(buffer.len)))
    }

    private static func error(_ status: Int32, _ detail: [UInt8] = []) -> AcmeRuntimeError {
        let name = String(cString: acme_status_name(status))
        let message = detail.isEmpty ? name : "\(name): \(String(decoding: detail, as: UTF8.self))"
        return .native(status: status, message: message)
    }

    // -------------------------------------------------------------- calls

    public func callSync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) throws -> [UInt8] {
        var response = acme_buffer(ptr: nil, len: 0)
        let status = request.withUnsafeBufferPointer { p in
            acme_call_sync(object, methodId, acme_bytes(ptr: p.baseAddress, len: p.count), &response)
        }
        let bytes = NativeBackend.consume(response)
        guard status == AcmeAbi.ok else { throw NativeBackend.error(status, bytes) }
        return bytes
    }

    public func callAsync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) async throws -> [UInt8] {
        try Task.checkCancellation() // a cancelled task never starts the operation
        var future: UInt64 = 0
        let status = request.withUnsafeBufferPointer { p in
            acme_call_async(object, methodId, acme_bytes(ptr: p.baseAddress, len: p.count), &future)
        }
        guard status == AcmeAbi.ok else { throw NativeBackend.error(status) }
        defer { _ = acme_future_drop(future) }
        try await NativeBackend.whenReady(future)
        let (taken, bytes) = NativeBackend.take(future)
        switch taken {
        case AcmeAbi.ok: return bytes
        case AcmeAbi.errCancelled: throw CancellationError()
        default: throw NativeBackend.error(taken, bytes)
        }
    }

    private static func take(_ future: UInt64) -> (Int32, [UInt8]) {
        var buffer = acme_buffer(ptr: nil, len: 0)
        let status = acme_future_take(future, &buffer)
        return (status, consume(buffer))
    }

    // ------------------------------------------------------------ futures

    /// Suspends until the native future is ready. Swift Task cancellation is
    /// forwarded with acme_future_cancel; we still wait for the core's
    /// completion, so the outcome (value or cancelled) is always observed.
    private static func whenReady(_ future: UInt64) async throws {
        let status: Int32 = await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Int32, Never>) in
                let box = Unmanaged.passRetained(ReadyBox(continuation)).toOpaque()
                let status = acme_future_subscribe(future, onReady, box)
                if status != AcmeAbi.ok {
                    Unmanaged<ReadyBox>.fromOpaque(box).release() // callback will never run
                    continuation.resume(returning: status)
                }
            }
        } onCancel: {
            _ = acme_future_cancel(future)
        }
        guard status == AcmeAbi.ok || status == AcmeAbi.ready else { throw error(status) }
    }

    // ------------------------------------------------------------ streams

    public func streamNext(_ stream: UInt64) async throws -> [UInt8]? {
        var future: UInt64 = 0
        let status = acme_stream_next(stream, &future)
        guard status == AcmeAbi.ok else { throw NativeBackend.error(status) }
        defer { _ = acme_future_drop(future) }
        try await NativeBackend.whenReady(future)
        let (taken, bytes) = NativeBackend.take(future)
        switch taken {
        case AcmeAbi.ok: return bytes
        case AcmeAbi.streamEnd: return nil
        case AcmeAbi.errCancelled:
            if Task.isCancelled { throw CancellationError() }
            return nil // cancelled by the core, e.g. the client was closed
        default: throw NativeBackend.error(taken, bytes)
        }
    }

    public func streamClose(_ stream: UInt64) {
        _ = acme_stream_cancel(stream)
        _ = acme_stream_drop(stream)
    }

    // ------------------------------------------------------------ objects

    public func dropObject(_ object: UInt64) {
        _ = acme_object_drop(object)
    }
}

/// Placeholder for running the same contract as a WebAssembly component; see
/// docs/COMPONENT_MODEL.md. Generated client classes stay identical.
public final class ComponentModelBackend: ClientBackend, @unchecked Sendable {
    public var description: String { "component-model (not implemented in this PoC)" }
    public init() {}
    public func callSync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) throws -> [UInt8] { throw unsupported() }
    public func callAsync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) async throws -> [UInt8] { throw unsupported() }
    public func streamNext(_ stream: UInt64) async throws -> [UInt8]? { throw unsupported() }
    public func streamClose(_ stream: UInt64) {}
    public func dropObject(_ object: UInt64) {}
    private func unsupported() -> AcmeRuntimeError { .incompatibleLibrary("component model backend is an extension point") }
}
