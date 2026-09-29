// Method-id dispatch for the C# core. Payloads are (de)serialised with the
// *generated* WireCodec (the same code the C# SDK uses on the other side), so
// only the "which method does what" table is written by hand here — a C#
// core emitter for acme-wit-gen could generate it like the Rust one.
using Acme.Client;
using Acme.Client.Runtime;

namespace Acme.Core;

static class Dispatch
{
    // Method ids: wit/abi-ids.lock
    private const uint Ctor = 0x0100, State = 0x0101, Connect = 0x0102, Disconnect = 0x0103,
        GetCustomer = 0x0104, CreateCustomer = 0x0105, SendCommand = 0x0106, Events = 0x0107;

    private static MsgPackReader Params(byte[] request, int n)
    {
        var r = new MsgPackReader(request);
        var got = r.ReadArrayHeader();
        if (got != n) throw new AcmeProtocolException($"expected {n} parameters, found {got}");
        return r;
    }

    private static void Done(MsgPackReader r)
    {
        if (r.Remaining != 0) throw new AcmeProtocolException($"{r.Remaining} trailing bytes after payload");
    }

    /// <summary>result&lt;T, client-error&gt; → [0, T] | [1, E]</summary>
    private static byte[] Result(ClientError? error, Action<MsgPackWriter> ok) => Wire.Encode(w =>
    {
        w.WriteArrayHeader(2);
        if (error is not null)
        {
            w.WriteUInt(1);
            WireCodec.WriteClientError(w, error);
        }
        else
        {
            w.WriteUInt(0);
            ok(w);
        }
    });

    private static readonly Action<MsgPackWriter> Unit = w => w.WriteNil();

    public static (int, byte[]?) Sync(ulong obj, uint method, byte[] request)
    {
        switch (method)
        {
            case Ctor:
            {
                if (obj != 0) return (Status.InvalidHandle, null);
                var r = Params(request, 1);
                var options = WireCodec.ReadClientOptions(r);
                Done(r);
                var handle = Handles.Add(Handles.Object, new Client(options));
                return (Status.Ok, Wire.Encode(w => w.WriteUInt(handle)));
            }
            case State or Events:
            {
                if (Handles.Get<Client>(obj) is not { } c) return (Status.InvalidHandle, null);
                Done(Params(request, 0));
                if (method == State) return (Status.Ok, Wire.Encode(w => WireCodec.WriteConnectionState(w, c.State)));
                var (reader, unsubscribe) = c.Subscribe();
                var stream = Handles.Add(Handles.Stream, new NativeStream(reader, unsubscribe));
                return (Status.Ok, Wire.Encode(w => w.WriteUInt(stream)));
            }
            case Connect or Disconnect or GetCustomer or CreateCustomer or SendCommand:
                return (Handles.Get<Client>(obj) is null ? Status.InvalidHandle : Status.Unsupported, null);
            default:
                return (Status.UnknownMethod, null);
        }
    }

    public static (int, Func<CancellationToken, Task<byte[]>>?) Async(ulong obj, uint method, byte[] request)
    {
        if (method is Ctor or State or Events) return (Status.Unsupported, null);
        if (method is < Connect or > SendCommand) return (Status.UnknownMethod, null);
        if (Handles.Get<Client>(obj) is not { } c) return (Status.InvalidHandle, null);
        switch (method)
        {
            case Connect:
                Done(Params(request, 0));
                return (Status.Ok, _ => Task.FromResult(Result(c.Connect(), Unit)));
            case Disconnect:
                Done(Params(request, 0));
                return (Status.Ok, _ => { c.Disconnect(); return Task.FromResult(Result(null, Unit)); });
            case GetCustomer:
            {
                var r = Params(request, 1);
                var id = r.ReadString();
                Done(r);
                return (Status.Ok, _ =>
                {
                    var (customer, error) = c.GetCustomer(id);
                    return Task.FromResult(Result(error, w => WireCodec.WriteCustomer(w, customer!)));
                });
            }
            case CreateCustomer:
            {
                var r = Params(request, 2);
                var name = r.ReadString();
                var email = r.TryReadNil() ? null : r.ReadString();
                Done(r);
                return (Status.Ok, _ =>
                {
                    var (customer, error) = c.CreateCustomer(name, email);
                    return Task.FromResult(Result(error, w => WireCodec.WriteCustomer(w, customer!)));
                });
            }
            default: // SendCommand
            {
                var r = Params(request, 1);
                var command = WireCodec.ReadCommand(r);
                Done(r);
                return (Status.Ok, async cancelled =>
                {
                    var (payload, error) = await c.SendCommandAsync(command.Name, command.Payload, cancelled).ConfigureAwait(false);
                    return Result(error, w => WireCodec.WriteCommandResult(w, new CommandResult(command.Name, payload!)));
                });
            }
        }
    }
}
