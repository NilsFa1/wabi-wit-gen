// The gematik:zero C# core: identity and resource factory (ICore is generated).
namespace Wabi.Core;

sealed class Core : ICore
{
    public string CoreInfo => "gematik-zero-csharp-core/0.1.0 (RFC 9449 in C#, NativeAOT)";

    public IDpopSigner CreateDpopSigner(string keyId) => new DpopSigner(keyId);
}
