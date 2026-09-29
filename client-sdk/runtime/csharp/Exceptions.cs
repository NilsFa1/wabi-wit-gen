// Hand-written, generic exception hierarchy.
#nullable enable
using System;

namespace Acme.Client.Runtime
{
    /// <summary>Base class of every exception thrown by an Acme SDK.</summary>
    public class AcmeException : Exception
    {
        public AcmeException(string message, Exception? inner = null) : base(message, inner) { }
    }

    /// <summary>The core broke the ABI contract or returned an undecodable payload.</summary>
    public sealed class AcmeProtocolException : AcmeException
    {
        public AcmeProtocolException(string message) : base(message) { }
    }

    /// <summary>An ABI-level failure (never a domain error), e.g. an invalid handle or a core bug.</summary>
    public sealed class AcmeNativeException : AcmeException
    {
        public int Status { get; }
        public AcmeNativeException(int status, string statusName, string? detail)
            : base(string.IsNullOrEmpty(detail) ? statusName : $"{statusName}: {detail}") => Status = status;
    }

    /// <summary>The loaded native library does not implement a compatible ABI/contract version.</summary>
    public sealed class AcmeIncompatibleLibraryException : AcmeException
    {
        public AcmeIncompatibleLibraryException(string message) : base(message) { }
    }

    /// <summary>A result error whose type is not a named contract type.</summary>
    public sealed class AcmeCallFailedException : AcmeException
    {
        public object? Error { get; }
        public AcmeCallFailedException(object? error) : base($"call failed: {error}") => Error = error;
    }
}

