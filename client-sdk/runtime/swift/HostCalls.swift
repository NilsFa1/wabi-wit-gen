// Hand-written, generic: ABI 1.1 host calls (ABI.md §4a). The core calls app
// code for the functions of WIT *imports*; the generated <Package>Host enum
// decodes the request and calls the app's protocol implementation.

import CAcmeClient
import Foundation

/// Generated per package: method id + encoded request -> encoded answer.
public typealias HostDispatch = @Sendable (UInt32, [UInt8]) async throws -> [UInt8]

private final class HostRegistry: @unchecked Sendable {
    private let lock = NSLock()
    private var dispatch: HostDispatch?

    func set(_ d: HostDispatch?) { lock.lock(); dispatch = d; lock.unlock() }
    func get() -> HostDispatch? { lock.lock(); defer { lock.unlock() }; return dispatch }
}

private let registry = HostRegistry()

/// Invoked by the core on one of ITS threads. Copies the request (valid until
/// the reply anyway), starts a detached task and returns immediately; the
/// answer goes back with acme_host_reply exactly once.
private let onHostCall: acme_host_handler = { _, methodId, request, length, call in
    let bytes: [UInt8] = (request != nil && length > 0) ? Array(UnsafeBufferPointer(start: request, count: Int(length))) : []
    Task.detached { await HostCalls.answer(methodId, bytes, call) }
}

public enum HostCalls {
    /// Installs the dispatcher for all imports of this package (one per process).
    public static func install(_ dispatch: @escaping HostDispatch) throws {
        let backend = try NativeBackend.shared.get()
        guard backend.info.capabilities & AcmeAbi.capHostCalls != 0 else {
            throw AcmeRuntimeError.incompatibleLibrary("core '\(backend.info.coreDescription)' does not support host calls (ABI 1.1)")
        }
        registry.set(dispatch)
        let status = acme_host_register(onHostCall, nil)
        guard status == AcmeAbi.ok else { throw AcmeRuntimeError.native(status: status, message: "host_register failed") }
    }

    /// Removes the handler; later host calls fail inside the core.
    public static func uninstall() {
        _ = acme_host_register(nil, nil)
        registry.set(nil)
    }

    static func answer(_ methodId: UInt32, _ request: [UInt8], _ call: UInt64) async {
        var status = AcmeAbi.ok
        var response: [UInt8]
        do {
            guard let dispatch = registry.get() else {
                throw AcmeRuntimeError.native(status: AcmeAbi.errUnsupported, message: "no host handler")
            }
            response = try await dispatch(methodId, request)
        } catch AcmeRuntimeError.native(let s, let message) {
            status = s
            response = Array(message.utf8)
        } catch {
            status = AcmeAbi.errInternal // an app bug: reported to the core, never lost
            response = Array(String(describing: error).utf8)
        }
        response.withUnsafeBufferPointer { p in
            _ = acme_host_reply(call, status, acme_bytes(ptr: p.baseAddress, len: p.count))
        }
    }
}
