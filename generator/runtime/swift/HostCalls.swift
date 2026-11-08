// Hand-written, generic: host calls (ABI 1.2, ABI.md §4a). The core calls app
// code for the functions of WIT *imports*; the generated <Package>Host enum
// decodes the request and calls the app's protocol implementation.
//
// An implementation is registered for the whole package or for one instance
// (object); the core names the instance with every call. If the core gives up
// on a call, the task running the app code is cancelled.

import CWabiClient
import Foundation

/// Generated per package: method id + encoded request -> encoded answer.
public typealias HostDispatch = @Sendable (UInt32, [UInt8]) async throws -> [UInt8]

private final class HostRegistry: @unchecked Sendable {
    private let lock = NSLock()
    private var dispatch: HostDispatch?
    private var perObject: [UInt64: HostDispatch] = [:]
    private var registered = false
    /// Calls the app is working on: cancelled when the core gives up on them.
    private var running: [UInt64: Task<Void, Never>] = [:]

    /// Changes the registrations and registers with the core while there is any
    /// dispatcher, unregisters when the last one is gone.
    func update(_ change: (inout HostDispatch?, inout [UInt64: HostDispatch]) -> Void) throws {
        lock.lock()
        change(&dispatch, &perObject)
        let wanted = dispatch != nil || !perObject.isEmpty
        let was = registered
        registered = wanted
        lock.unlock()
        guard wanted != was else { return }
        if !wanted {
            // Returns once no handler invocation is running; later host calls fail inside the core.
            _ = wabi_host_register2(nil, nil, nil)
            return
        }
        let status = wabi_host_register2(onHostCall, onHostCancel, nil)
        guard status == WabiAbi.ok else {
            lock.lock(); registered = false; lock.unlock()
            throw WabiRuntimeError.native(status: status, message: "host_register2 failed")
        }
    }

    var hasPerObject: Bool { lock.lock(); defer { lock.unlock() }; return !perObject.isEmpty }

    /// Starts the app code for one call. The implementation is chosen now: a
    /// call keeps it even if it is replaced meanwhile.
    func start(object: UInt64, _ methodId: UInt32, _ request: [UInt8], _ call: UInt64) {
        lock.lock()
        let target = perObject[object] ?? dispatch
        // The task removes itself from `running` under the same lock, i.e. after this insert.
        running[call] = Task.detached { await HostCalls.answer(target, methodId, request, call) }
        lock.unlock()
    }

    func finished(_ call: UInt64) { lock.lock(); running[call] = nil; lock.unlock() }

    func cancel(_ call: UInt64) {
        lock.lock()
        let task = running[call]
        lock.unlock()
        task?.cancel()
    }
}

private let registry = HostRegistry()

/// Invoked by the core on one of ITS threads. Copies the request (valid until
/// the reply anyway), starts a detached task and returns immediately; the
/// answer goes back with wabi_host_reply exactly once.
private let onHostCall: wabi_host_handler2 = { _, object, methodId, request, length, call in
    let bytes: [UInt8] = (request != nil && length > 0) ? Array(UnsafeBufferPointer(start: request, count: Int(length))) : []
    registry.start(object: object, methodId, bytes, call)
}

/// Invoked by the core on one of ITS threads: it no longer waits for the call.
private let onHostCancel: wabi_host_cancel = { _, call in registry.cancel(call) }

public enum HostCalls {
    private static func ensureSupported() throws {
        let backend = try NativeBackend.shared.get()
        guard backend.info.capabilities & WabiAbi.capHostCalls2 != 0 else {
            throw WabiRuntimeError.incompatibleLibrary("core '\(backend.info.coreDescription)' does not support host calls (ABI 1.2)")
        }
    }

    /// Installs the dispatcher for all imports of this package (one per process).
    public static func install(_ dispatch: @escaping HostDispatch) throws {
        try ensureSupported()
        try registry.update { all, _ in all = dispatch }
    }

    /// Installs the dispatcher for the calls the core makes on behalf of one
    /// object; it takes precedence over the package-wide one and goes away
    /// with the object.
    public static func install(object: UInt64, _ dispatch: @escaping HostDispatch) throws {
        try ensureSupported()
        try registry.update { _, perObject in perObject[object] = dispatch }
    }

    /// Removes the package-wide dispatcher; calls already running finish with it.
    public static func uninstall() {
        try? registry.update { all, _ in all = nil }
    }

    /// Removes the dispatcher of one object (no-op if it has none).
    public static func uninstall(object: UInt64) {
        guard registry.hasPerObject else { return } // the common case when objects are dropped
        try? registry.update { _, perObject in perObject[object] = nil }
    }

    static func answer(_ dispatch: HostDispatch?, _ methodId: UInt32, _ request: [UInt8], _ call: UInt64) async {
        var status = WabiAbi.ok
        var response: [UInt8]
        do {
            guard let dispatch else {
                throw WabiRuntimeError.native(status: WabiAbi.errUnsupported, message: "no host handler")
            }
            response = try await dispatch(methodId, request)
        } catch WabiRuntimeError.native(let s, let message) {
            status = s
            response = Array(message.utf8)
        } catch is CancellationError {
            status = WabiAbi.errCancelled // the core discards the answer anyway
            response = []
        } catch {
            status = WabiAbi.errInternal // an app bug: reported to the core, never lost
            response = Array(String(describing: error).utf8)
        }
        registry.finished(call)
        response.withUnsafeBufferPointer { p in
            _ = wabi_host_reply(call, status, wabi_bytes(ptr: p.baseAddress, len: p.count))
        }
    }
}
