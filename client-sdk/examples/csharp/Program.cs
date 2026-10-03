// Plain application code: only the public, generated SDK surface is used.
// Which core implements it (Rust, Go, Zig) is decided by the native library
// that gets loaded — this file never changes.
using System.Text;
using Wabi.Client;

var options = new ClientOptions(
    Endpoint: args.FirstOrDefault() ?? "mem://example",
    ApiKey: "demo-key",
    RequestTimeoutMs: 5_000);

await using var client = new WabiClient(options);

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
