// Hand-written, generic.

/// Runtime-level failures. Errors declared by the WIT contract are thrown directly.
public enum WabiRuntimeError: Error, Equatable, Sendable {
    /// The core broke the ABI contract or returned an undecodable payload.
    case protocolError(String)
    /// An ABI-level failure (never a domain error), e.g. an invalid handle.
    case native(status: Int32, message: String)
    /// The native core is missing or implements an incompatible ABI/contract version.
    case incompatibleLibrary(String)
    /// A result error whose type is not a named contract type.
    case callFailed(String)
    /// The object was closed.
    case closed
}
