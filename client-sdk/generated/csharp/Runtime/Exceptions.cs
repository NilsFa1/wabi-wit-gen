// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic exception hierarchy.
#nullable enable
using System;

namespace Wabi.Client.Runtime
{
    /// <summary>Base class of every exception thrown by an Wabi SDK.</summary>
    public class WabiException : Exception
    {
        public WabiException(string message, Exception? inner = null) : base(message, inner) { }
    }

    /// <summary>The core broke the ABI contract or returned an undecodable payload.</summary>
    public sealed class WabiProtocolException : WabiException
    {
        public WabiProtocolException(string message) : base(message) { }
    }

    /// <summary>An ABI-level failure (never a domain error), e.g. an invalid handle or a core bug.</summary>
    public sealed class WabiNativeException : WabiException
    {
        public int Status { get; }
        public WabiNativeException(int status, string statusName, string? detail)
            : base(string.IsNullOrEmpty(detail) ? statusName : $"{statusName}: {detail}") => Status = status;
    }

    /// <summary>The loaded native library does not implement a compatible ABI/contract version.</summary>
    public sealed class WabiIncompatibleLibraryException : WabiException
    {
        public WabiIncompatibleLibraryException(string message) : base(message) { }
    }

    /// <summary>A result error whose type is not a named contract type.</summary>
    public sealed class WabiCallFailedException : WabiException
    {
        public object? Error { get; }
        public WabiCallFailedException(object? error) : base($"call failed: {error}") => Error = error;
    }
}

