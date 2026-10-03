// The wabi:client C# core: identity and resource factory (ICore is generated).
using Wabi.Client;

namespace Wabi.Core;

sealed class Core : ICore
{
    public string CoreInfo => "wabi-csharp-core/0.1.0 (NativeAOT, thread pool)";

    public IClient CreateClient(ClientOptions options) => new Client(options);
}
