// The C# core's business logic: plain async C#, no interop. Same observable
// behaviour as the Rust/Go cores against the in-process backend (mem://).
// It uses the *generated* contract types (Customer, ClientError, ...) directly.
using System.Threading.Channels;
using Acme.Client;

namespace Acme.Core;

sealed class Client : ICoreObject
{
    private readonly ClientOptions _options;
    private readonly object _gate = new();
    private ConnectionState _state = ConnectionState.Disconnected;
    private bool _closed;
    private CancellationTokenSource? _connection; // cancelled on disconnect / hang-up / close
    private readonly Dictionary<int, Channel<ClientEvent>> _subscribers = new();
    private int _nextSubscriber;
    private readonly Dictionary<string, Customer> _customers = new()
    {
        ["4711"] = new Customer("4711", "Erika Mustermann", "erika@example.com"),
    };
    private int _nextId = 5000;

    public Client(ClientOptions options) => _options = options;

    public ConnectionState State
    {
        get { lock (_gate) return _state; }
    }

    // Caller holds _gate. Slow subscribers lose events instead of blocking the core.
    private void Publish(ClientEvent e)
    {
        foreach (var ch in _subscribers.Values) ch.Writer.TryWrite(e);
    }

    private void SetState(ConnectionState s)
    {
        if (_state == s) return;
        _state = s;
        Publish(new ClientEvent.ConnectionStateChanged(s));
    }

    private void DropConnection()
    {
        _connection?.Cancel();
        _connection = null;
        SetState(ConnectionState.Disconnected);
    }

    private CancellationToken? Live()
    {
        lock (_gate) return _connection is { IsCancellationRequested: false } c ? c.Token : null;
    }

    public ClientError? Connect()
    {
        lock (_gate)
        {
            if (_closed) return new ClientError.Disconnected();
            if (_connection is { IsCancellationRequested: false }) return null;
            SetState(ConnectionState.Connecting);
            ClientError? error = _options.Endpoint switch
            {
                "mem://unreachable" => new ClientError.NetworkError("connection refused"),
                var e when !e.StartsWith("mem://", StringComparison.Ordinal) =>
                    new ClientError.NetworkError($"csharp core supports mem:// endpoints only: {e}"),
                _ when _options.ApiKey != "demo-key" => new ClientError.Unauthorized(),
                _ => null,
            };
            if (error is not null)
            {
                SetState(ConnectionState.Disconnected);
                return error;
            }
            _connection = new CancellationTokenSource();
            SetState(ConnectionState.Connected);
            return null;
        }
    }

    public void Disconnect()
    {
        lock (_gate) DropConnection();
    }

    public (Customer?, ClientError?) GetCustomer(string id)
    {
        if (Live() is null) return (null, new ClientError.Disconnected());
        lock (_gate) return _customers.TryGetValue(id, out var c) ? (c, null) : (null, new ClientError.NotFound());
    }

    public (Customer?, ClientError?) CreateCustomer(string name, string? email)
    {
        if (Live() is null) return (null, new ClientError.Disconnected());
        if (string.IsNullOrWhiteSpace(name)) return (null, new ClientError.ProtocolError("name must not be empty"));
        lock (_gate)
        {
            var c = new Customer((++_nextId).ToString(), name, email);
            _customers[c.Id] = c;
            Publish(new ClientEvent.CustomerUpdated(c));
            return (c, null);
        }
    }

    /// <param name="cancelled">host cancellation (acme_future_cancel)</param>
    public async Task<(byte[]?, ClientError?)> SendCommandAsync(string name, byte[] payload, CancellationToken cancelled)
    {
        if (Live() is not { } connection) return (null, new ClientError.Disconnected());
        switch (name)
        {
            case "echo":
                return (payload, null);
            case "broadcast":
                lock (_gate) Publish(new ClientEvent.MessageReceived(System.Text.Encoding.UTF8.GetString(payload)));
                return (Array.Empty<byte>(), null);
            case "sleep":
                int.TryParse(System.Text.Encoding.UTF8.GetString(payload), out var ms);
                using (var timeout = new CancellationTokenSource())
                using (var any = CancellationTokenSource.CreateLinkedTokenSource(cancelled, connection, timeout.Token))
                {
                    if (_options.RequestTimeoutMs > 0) timeout.CancelAfter((int)_options.RequestTimeoutMs);
                    try
                    {
                        await Task.Delay(ms, any.Token).ConfigureAwait(false);
                        return (payload, null);
                    }
                    catch (OperationCanceledException) when (timeout.IsCancellationRequested && !cancelled.IsCancellationRequested)
                    {
                        return (null, new ClientError.NetworkError("request timed out"));
                    }
                    catch (OperationCanceledException)
                    {
                        return (null, new ClientError.Disconnected()); // discarded if the host cancelled
                    }
                }
            case "fail":
                return (null, new ClientError.ProtocolError("command failed on purpose"));
            case "hang-up":
                lock (_gate) DropConnection();
                return (null, new ClientError.Disconnected());
            default:
                return (null, new ClientError.ProtocolError($"unknown command `{name}`"));
        }
    }

    public (ChannelReader<ClientEvent> Reader, Action Unsubscribe) Subscribe()
    {
        var ch = Channel.CreateBounded<ClientEvent>(new BoundedChannelOptions(1024) { FullMode = BoundedChannelFullMode.DropWrite });
        lock (_gate)
        {
            if (_closed)
            {
                ch.Writer.Complete();
                return (ch.Reader, () => { });
            }
            var id = _nextSubscriber++;
            _subscribers[id] = ch;
            return (ch.Reader, () =>
            {
                lock (_gate)
                    if (_subscribers.Remove(id)) ch.Writer.TryComplete();
            });
        }
    }

    /// <summary>Resource destructor: fails in-flight work, ends all streams.</summary>
    public void Close()
    {
        lock (_gate)
        {
            if (_closed) return;
            DropConnection();
            _closed = true;
            foreach (var ch in _subscribers.Values) ch.Writer.TryComplete();
            _subscribers.Clear();
        }
    }
}
