// Plain application code: only the public, generated SDK surface is used.
// Which core implements it (Rust, Go) is decided by the native library
// that gets loaded — this file never changes.
using System.Security.Cryptography;
using System.Text;
using Demo.Client;

// The app keeps the signing key; the core asks for every signature (host calls).
ClientHost.Register(new AppKeys());

var options = new ClientOptions(
    Endpoint: args.FirstOrDefault() ?? "mem://example",
    ApiKey: "demo-key",
    RequestTimeoutMs: 5_000,
    KeyId: "app-key");

await using var client = new DemoClient(options);

using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(5));
var listener = Task.Run(async () =>
{
    await foreach (var evt in client.Events(cts.Token))
    {
        Console.WriteLine($"  event: {evt}");
        if (evt is ClientEvent.MessageReceived) break;
    }
});

await client.ConnectAsync();
Console.WriteLine($"backend verified our key: {await client.KeyFingerprintAsync()}");
Customer customer = await client.GetCustomerAsync("4711");
Console.WriteLine($"customer: {customer.Name} <{customer.Email}>");

try
{
    await client.GetCustomerAsync("0000");
}
catch (ClientException e) when (e.Error is ClientError.NotFound)
{
    Console.WriteLine("customer 0000: not found");
}

var created = await client.CreateCustomerAsync("Max Mustermann", email: null);
Console.WriteLine($"created: {created}");
await client.SendCommandAsync(new Command("broadcast", Encoding.UTF8.GetBytes("hello from C#")));

await listener;
await client.DisconnectAsync();
Console.WriteLine($"state: {client.State()}");

/// <summary>Stands in for a TPM / Windows Hello / HSM: the private key never leaves this class.</summary>
sealed class AppKeys : IHostKeys
{
    readonly ECDsa _key = ECDsa.Create(ECCurve.NamedCurves.nistP256);

    public Task<byte[]> PublicKeyAsync(string keyId, CancellationToken cancellationToken) =>
        Task.FromResult(_key.ExportSubjectPublicKeyInfo());

    public Task<byte[]> SignAsync(string keyId, byte[] digest, CancellationToken cancellationToken) =>
        Task.FromResult(_key.SignHash(digest, DSASignatureFormat.Rfc3279DerSequence));
}
