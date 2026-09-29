// Hand-written, generic: the seam between generated SDK code and whatever
// executes the contract (native core, WebAssembly component, test double).

import Foundation

public protocol ClientBackend: AnyObject, Sendable {
    var description: String { get }
    /// Synchronous WIT function (never blocks on I/O). Returns the encoded result.
    func callSync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) throws -> [UInt8]
    /// Async WIT function. Swift Task cancellation is forwarded to the core.
    func callAsync(_ object: UInt64, _ methodId: UInt32, _ request: [UInt8]) async throws -> [UInt8]
    /// Next stream item; `nil` = end of stream. Task cancellation abandons only this pull.
    func streamNext(_ stream: UInt64) async throws -> [UInt8]?
    /// Stops and releases a stream handle. Called exactly once per stream.
    func streamClose(_ stream: UInt64)
    func dropObject(_ object: UInt64)
}

/// Owns one object handle; releases it exactly once (close() or deinit).
public final class ObjectHandle: @unchecked Sendable {
    private let backend: ClientBackend
    private var raw: UInt64
    private let lock = NSLock()

    public init(backend: ClientBackend, value: UInt64) throws {
        guard value != 0 else { throw AcmeRuntimeError.protocolError("core returned the null handle") }
        self.backend = backend
        self.raw = value
    }

    public func value() throws -> UInt64 {
        lock.lock()
        let v = raw
        lock.unlock()
        guard v != 0 else { throw AcmeRuntimeError.closed }
        return v
    }

    public func close() {
        lock.lock()
        let v = raw
        raw = 0
        lock.unlock()
        if v != 0 { backend.dropObject(v) }
    }

    deinit { close() }
}

/// `for try await item in stream` over an ABI stream handle.
///
/// Pull-based: the core is asked for the next item only when the consumer calls
/// `next()`, so there is no unbounded buffering. Leaving the loop (break, throw,
/// or task cancellation) releases the native stream when the iterator is freed.
public struct AcmeStream<Element>: AsyncSequence {
    public typealias AsyncIterator = Iterator

    let open: () throws -> (ClientBackend, UInt64)
    let decode: ([UInt8]) throws -> Element

    public init(open: @escaping () throws -> (ClientBackend, UInt64), decode: @escaping ([UInt8]) throws -> Element) {
        self.open = open
        self.decode = decode
    }

    public func makeAsyncIterator() -> Iterator { Iterator(open: open, decode: decode) }

    public final class Iterator: AsyncIteratorProtocol {
        private let open: () throws -> (ClientBackend, UInt64)
        private let decode: ([UInt8]) throws -> Element
        private var stream: (backend: ClientBackend, handle: UInt64)?
        private var finished = false

        init(open: @escaping () throws -> (ClientBackend, UInt64), decode: @escaping ([UInt8]) throws -> Element) {
            self.open = open
            self.decode = decode
        }

        public func next() async throws -> Element? {
            if finished { return nil }
            do {
                if stream == nil {
                    let (backend, handle) = try open() // the native subscription starts on first pull
                    stream = (backend, handle)
                }
                guard let s = stream, let bytes = try await s.backend.streamNext(s.handle) else {
                    finish()
                    return nil
                }
                return try decode(bytes)
            } catch {
                finish()
                throw error
            }
        }

        private func finish() {
            finished = true
            if let s = stream {
                s.backend.streamClose(s.handle)
                stream = nil
            }
        }

        deinit {
            if let s = stream { s.backend.streamClose(s.handle) }
        }
    }
}
