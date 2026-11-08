// Hand-written, generic: executes contract calls through the Wabi native ABI
// through the package's generated C module. No per-method code here.

import CWabiClient

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
        WabiAbi.capSyncCalls | WabiAbi.capAsyncCalls | WabiAbi.capStreams | WabiAbi.capPayloadMsgpack1

    /// ABI: same major, core minor >= SDK minor. Contract: same package + major, core minor >= SDK minor.
    public func ensureCompatible() throws {
        guard abiMajor == WabiAbi.major, abiMinor >= WabiAbi.minor else {
            throw WabiRuntimeError.incompatibleLibrary("core '\(coreDescription)' implements ABI \(abiMajor).\(abiMinor), SDK needs \(WabiAbi.major).\(WabiAbi.minor)+")
        }
        let needed = CoreInfo.required | (WabiAbi.requiresHostCalls ? WabiAbi.capHostCalls : 0)
        guard capabilities & needed == needed else {
            throw WabiRuntimeError.incompatibleLibrary("core '\(coreDescription)' lacks required capabilities")
        }
        let parts = contractId.split(separator: "@", maxSplits: 1)
        let version = parts.count == 2 ? parts[1].split(separator: ".").compactMap { Int($0) } : []
        guard parts.first.map(String.init) == Contract.package, version.count >= 2,
              version[0] == Contract.major, version[1] >= Contract.minor else {
            throw WabiRuntimeError.incompatibleLibrary("core implements '\(contractId)', SDK was generated for '\(Contract.id)'")
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
private let onReady: wabi_future_callback = { userData, _ in
    guard let userData else { return }
    Unmanaged<ReadyBox>.fromOpaque(userData).takeRetainedValue().continuation.resume(returning: WabiAbi.ok)
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
            abiVersion: wabi_abi_version(),
            capabilities: wabi_abi_capabilities(),
            contractId: String(cString: wabi_contract_id()),
            coreDescription: String(cString: wabi_core_info()))
    }

    // ------------------------------------------------------------ buffers

    /// Copies a core-owned buffer into Swift memory and frees it (always).
    private static func consume(_ buffer: wabi_buffer) -> [UInt8] {
        defer { wabi_buffer_free(buffer) }
        guard let ptr = buffer.ptr, buffer.len > 0 else { return [] }
        return Array(UnsafeBufferPointer(start: ptr, count: Int(buffer.len)))
    }

    private static func error(_ status: Int32, _ detail: [UInt8] = []) -> WabiRuntimeError {
        let name = String(cString: wabi_status_name(status))
        let message = detail.isEmpty ? name : "\(name): \(String(decoding: detail, as: UTF8.self))"
        return .native(status: status, message: message)
    }

    // -------------------------------------------------------------- calls

    public func callSync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) throws -> [UInt8] {
        var response = wabi_buffer(ptr: nil, len: 0)
        let status = request.withUnsafeBufferPointer { p in
            wabi_call_sync(object, methodId, wabi_bytes(ptr: p.baseAddress, len: p.count), &response)
        }
        let bytes = NativeBackend.consume(response)
        guard status == WabiAbi.ok else { throw NativeBackend.error(status, bytes) }
        return bytes
    }

    public func callAsync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) async throws -> [UInt8] {
        try Task.checkCancellation() // a cancelled task never starts the operation
        var future: UInt64 = 0
        let status = request.withUnsafeBufferPointer { p in
            wabi_call_async(object, methodId, wabi_bytes(ptr: p.baseAddress, len: p.count), &future)
        }
        guard status == WabiAbi.ok else { throw NativeBackend.error(status) }
        defer { _ = wabi_future_drop(future) }
        try await NativeBackend.whenReady(future)
        let (taken, bytes) = NativeBackend.take(future)
        switch taken {
        case WabiAbi.ok: return bytes
        case WabiAbi.errCancelled: throw CancellationError()
        default: throw NativeBackend.error(taken, bytes)
        }
    }

    private static func take(_ future: UInt64) -> (Int32, [UInt8]) {
        var buffer = wabi_buffer(ptr: nil, len: 0)
        let status = wabi_future_take(future, &buffer)
        return (status, consume(buffer))
    }

    // ------------------------------------------------------------ futures

    /// Suspends until the native future is ready. Swift Task cancellation is
    /// forwarded with wabi_future_cancel; we still wait for the core's
    /// completion, so the outcome (value or cancelled) is always observed.
    private static func whenReady(_ future: UInt64) async throws {
        let status: Int32 = await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Int32, Never>) in
                let box = Unmanaged.passRetained(ReadyBox(continuation)).toOpaque()
                let status = wabi_future_subscribe(future, onReady, box)
                if status != WabiAbi.ok {
                    Unmanaged<ReadyBox>.fromOpaque(box).release() // callback will never run
                    continuation.resume(returning: status)
                }
            }
        } onCancel: {
            _ = wabi_future_cancel(future)
        }
        guard status == WabiAbi.ok || status == WabiAbi.ready else { throw error(status) }
    }

    // ------------------------------------------------------------ streams

    public func streamNext(_ stream: UInt64) async throws -> [UInt8]? {
        var future: UInt64 = 0
        let status = wabi_stream_next(stream, &future)
        guard status == WabiAbi.ok else { throw NativeBackend.error(status) }
        defer { _ = wabi_future_drop(future) }
        try await NativeBackend.whenReady(future)
        let (taken, bytes) = NativeBackend.take(future)
        switch taken {
        case WabiAbi.ok: return bytes
        case WabiAbi.streamEnd: return nil
        case WabiAbi.errCancelled:
            if Task.isCancelled { throw CancellationError() }
            return nil // cancelled by the core, e.g. the client was closed
        default: throw NativeBackend.error(taken, bytes)
        }
    }

    public func streamClose(_ stream: UInt64) {
        _ = wabi_stream_cancel(stream)
        _ = wabi_stream_drop(stream)
    }

    // ------------------------------------------------------------ objects

    public func dropObject(_ object: UInt64) {
        HostCalls.uninstall(object: object) // an implementation registered for this object goes with it
        _ = wabi_object_drop(object)
    }
}

/// Placeholder for running the same contract as a WebAssembly component.
/// Generated client classes stay identical.
public final class ComponentModelBackend: ClientBackend, @unchecked Sendable {
    public var description: String { "component-model (not implemented in this PoC)" }
    public init() {}
    public func callSync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) throws -> [UInt8] { throw unsupported() }
    public func callAsync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) async throws -> [UInt8] { throw unsupported() }
    public func streamNext(_ stream: UInt64) async throws -> [UInt8]? { throw unsupported() }
    public func streamClose(_ stream: UInt64) {}
    public func dropObject(_ object: UInt64) {}
    private func unsupported() -> WabiRuntimeError { .incompatibleLibrary("component model backend is an extension point") }
}
