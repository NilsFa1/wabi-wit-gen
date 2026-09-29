// Variant 4: a C# application using the generated SDK against the native library.
using System.Diagnostics;
using Gematik.Pki;

var dir = Environment.GetEnvironmentVariable("PKI_FIXTURES") ?? "../tests/fixtures";
var tsl = File.ReadAllBytes(Path.Combine(dir, "tsl.xml"));
var cert = File.ReadAllBytes(Path.Combine(dir, "valid.der"));
var aut = new[] { CertificateProfile.CHciAutEcc };
const int Warmup = 3000, N = 5000, Threads = 8;

Console.WriteLine($"== C# SDK -> native library (.NET {Environment.Version})");
var sw = Stopwatch.StartNew();
var v = new CertificateVerifier("Bench", tsl);
await v.VerifyAsync(cert, aut, false);
Console.WriteLine($"{"first verify (incl. load + TSL)",-34} {sw.Elapsed.TotalMilliseconds,8:F1} ms");

async Task Latency(string name, Func<Task> op)
{
    for (var i = 0; i < Warmup; i++) await op();
    var t = new double[N];
    for (var i = 0; i < N; i++)
    {
        var s = Stopwatch.GetTimestamp();
        await op();
        t[i] = Stopwatch.GetElapsedTime(s).TotalMicroseconds;
    }
    Array.Sort(t);
    Console.WriteLine($"{name,-34} mean {t.Average(),8:F1} µs   p50 {t[N / 2],8:F1} µs   p99 {t[N * 99 / 100],8:F1} µs");
}

await Latency("verify (valid certificate)", () => v.VerifyAsync(cert, aut, false));
await Latency("rejected call (no profiles)", async () =>
{
    try { await v.VerifyAsync(cert, Array.Empty<CertificateProfile>(), false); } catch (PkiException) { }
});

var per = N * 2 / Threads;
async Task Worker() { for (var i = 0; i < per; i++) await v.VerifyAsync(cert, aut, false); }
await Task.WhenAll(Enumerable.Range(0, Threads).Select(_ => Task.Run(Worker)));
sw.Restart();
await Task.WhenAll(Enumerable.Range(0, Threads).Select(_ => Task.Run(Worker)));
Console.WriteLine($"{"verify throughput",-34} {per * Threads / sw.Elapsed.TotalSeconds,8:F0} ops/s ({Threads} callers)");
v.Dispose();
