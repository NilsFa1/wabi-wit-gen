// Package part of the C# core for acme:client: completes the generic exports
// of runtime/csharp-core (ABI.md §1.1 symbol prefix, metadata).
namespace Acme.Core;

static partial class Exports
{
    /// <summary>Package symbol prefix: acme:client -> acme_client_call_sync, ...</summary>
    private const string Prefix = "acme_client_";
    private const string ContractIdText = "acme:client@1.0.0";
    private const string CoreInfoText = "acme-csharp-core/0.1.0 (NativeAOT, thread pool)";
    private const bool HostCallsEnabled = false; // no WIT imports
}
