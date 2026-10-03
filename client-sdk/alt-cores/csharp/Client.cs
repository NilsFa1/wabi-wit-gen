// The C# core's business logic: plain async C#, no interop. Same observable
// behaviour as the Rust/Go cores against the in-process backend (mem://).
// It implements the generated IClient and uses the generated contract types
// (Customer, ClientError, ...) directly; domain errors are thrown as ClientException.
using System.Threading.Channels;
using Wabi.Client;

namespace Wabi.Core;

sealed class Client : IClient
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

    public ConnectionState State()
    {
        lock (_gate) return _state;
    }

    private static ClientException Fail(ClientError e) => new(e);

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

    public Task ConnectAsync(CancellationToken cancellationToken)
    {
        lock (_gate)
        {
            if (_closed) throw Fail(new ClientError.Disconnected());
            if (_connection is { IsCancellationRequested: false }) return Task.CompletedTask;
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
                throw Fail(error);
            }
            _connection = new CancellationTokenSource();
            SetState(ConnectionState.Connected);
            return Task.CompletedTask;
        }
    }

    public Task DisconnectAsync(CancellationToken cancellationToken)
    {
        lock (_gate) DropConnection();
        return Task.CompletedTask;
    }

    public Task<Customer> GetCustomerAsync(string id, CancellationToken cancellationToken)
    {
        if (Live() is null) throw Fail(new ClientError.Disconnected());
        lock (_gate) return _customers.TryGetValue(id, out var c) ? Task.FromResult(c) : throw Fail(new ClientError.NotFound());
    }

    public Task<Customer> CreateCustomerAsync(string name, string? email, CancellationToken cancellationToken)
    {
        if (Live() is null) throw Fail(new ClientError.Disconnected());
        if (string.IsNullOrWhiteSpace(name)) throw Fail(new ClientError.ProtocolError("name must not be empty"));
        lock (_gate)
        {
            var c = new Customer((++_nextId).ToString(), name, email);
            _customers[c.Id] = c;
            Publish(new ClientEvent.CustomerUpdated(c));
            return Task.FromResult(c);
        }
    }

    /// <param name="cancellationToken">host cancellation (wabi_future_cancel)</param>
    public async Task<CommandResult> SendCommandAsync(Command command, CancellationToken cancellationToken)
    {
        var (name, payload, cancelled) = (command.Name, command.Payload, cancellationToken);
        CommandResult Ok(byte[] p) => new(name, p);
        if (Live() is not { } connection) throw Fail(new ClientError.Disconnected());
        switch (name)
        {
            case "echo":
                return Ok(payload);
            case "broadcast":
                lock (_gate) Publish(new ClientEvent.MessageReceived(System.Text.Encoding.UTF8.GetString(payload)));
                return Ok(Array.Empty<byte>());
            case "sleep":
                int.TryParse(System.Text.Encoding.UTF8.GetString(payload), out var ms);
                using (var timeout = new CancellationTokenSource())
                using (var any = CancellationTokenSource.CreateLinkedTokenSource(cancelled, connection, timeout.Token))
                {
                    if (_options.RequestTimeoutMs > 0) timeout.CancelAfter((int)_options.RequestTimeoutMs);
                    try
                    {
                        await Task.Delay(ms, any.Token).ConfigureAwait(false);
                        return Ok(payload);
                    }
                    catch (OperationCanceledException) when (timeout.IsCancellationRequested && !cancelled.IsCancellationRequested)
                    {
                        throw Fail(new ClientError.NetworkError("request timed out"));
                    }
                    catch (OperationCanceledException)
                    {
                        throw Fail(new ClientError.Disconnected()); // discarded if the host cancelled
                    }
                }
            case "fail":
                throw Fail(new ClientError.ProtocolError("command failed on purpose"));
            case "hang-up":
                lock (_gate) DropConnection();
                throw Fail(new ClientError.Disconnected());
            default:
                throw Fail(new ClientError.ProtocolError($"unknown command `{name}`"));
        }
    }

    public CoreStream<ClientEvent> Events()
    {
        var ch = Channel.CreateBounded<ClientEvent>(new BoundedChannelOptions(1024) { FullMode = BoundedChannelFullMode.DropWrite });
        lock (_gate)
        {
            if (_closed)
            {
                ch.Writer.Complete();
                return new CoreStream<ClientEvent>(ch.Reader);
            }
            var id = _nextSubscriber++;
            _subscribers[id] = ch;
            return new CoreStream<ClientEvent>(ch.Reader, () =>
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
