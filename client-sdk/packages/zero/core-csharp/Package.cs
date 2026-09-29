// Package part of the C# core for gematik:zero: completes the generic exports
// of runtime/csharp-core (symbol prefix, metadata, host calls on).
namespace Acme.Core;

static partial class Exports
{
    /// <summary>Package symbol prefix: gematik:zero -> gematik_zero_call_sync, ...</summary>
    private const string Prefix = "gematik_zero_";
    private const string ContractIdText = "gematik:zero@0.1.0";
    private const string CoreInfoText = "gematik-zero-csharp-core/0.1.0 (RFC 9449 in C#, NativeAOT)";
    private const bool HostCallsEnabled = true; // WIT import host-keys
}
