using Demo.Greeter;

using var greeter = new DemoGreeter("Hallo");
var g = await greeter.GreetAsync("Nils");
Console.WriteLine($"{g.Text} ({g.Count})");
try
{
    await greeter.GreetAsync("");
}
catch (GreetException e)
{
    Console.WriteLine($"Fehler: {e.Error}");
}
