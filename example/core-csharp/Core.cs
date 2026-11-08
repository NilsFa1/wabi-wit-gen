// The demo:client C# core: identity and resource factory (ICore is generated).
using Demo.Client;

namespace Wabi.Core;

sealed class Core : ICore
{
    public string CoreInfo => "demo-csharp-core/0.2.0 (NativeAOT, HttpClient)";

    public ITransport CreateTransport(string endpoint) => new Transport(endpoint);

    public IClient CreateClient(ClientOptions options) => new Client(options);
}
