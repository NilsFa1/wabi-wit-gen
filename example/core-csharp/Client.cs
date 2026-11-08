// The C# core's business logic: plain async C#, no interop. It implements the
// generated IClient and uses the generated contract types (Customer,
// ClientError, ...) directly; domain errors are thrown as ClientException.
//
// A third, independent implementation of the example's contract and of its
// HTTP protocol (Session.cs), next to core-rust and core-go.
using System.Text.Json;
using System.Threading.Channels;
using Demo.Client;

namespace Wabi.Core;

/// <summary>
/// The connection pool to one backend, shared by every client that was given
/// it (resource <c>transport</c> of the contract).
/// </summary>
sealed class Transport(string endpoint) : ITransport
{
    private long _requests;

    public string Endpoint => endpoint;
    public void Count() => Interlocked.Increment(ref _requests);

    public ulong RequestCount() => (ulong)Interlocked.Read(ref _requests);

    public IClient OpenClient(ClientOptions options) => new Client(options, this);

    /// <summary><paramref name="other"/> was moved in by the host: nobody else can reach it any more.</summary>
    public void Absorb(ITransport other) => Interlocked.Add(ref _requests, (long)other.RequestCount());

    public void Close() { }
}

sealed class Client(ClientOptions options, Transport? shared = null) : IClient
{
    // What the next session sends its requests over; a private one unless the
    // host gave the client a shared transport.
    private Transport? _transport = shared;

    private readonly object _gate = new();
    private readonly SemaphoreSlim _lifecycle = new(1, 1); // serialises connect/disconnect
    private ConnectionState _state = ConnectionState.Disconnected;
    private bool _closed;
    private Session? _session; // ended on disconnect / hang-up / close
    private readonly Dictionary<int, Channel<ClientEvent>> _subscribers = new();
    private int _nextSubscriber;

    public ConnectionState State()
    {
        lock (_gate) return _state;
    }

    private static ClientException Fail(ClientError e) => new(e);

    // Slow subscribers lose events instead of blocking the core.
    private void Publish(ClientEvent e)
    {
        lock (_gate)
            foreach (var ch in _subscribers.Values) ch.Writer.TryWrite(e);
    }

    // Caller holds _gate.
    private void SetState(ConnectionState s)
    {
        if (_state == s) return;
        _state = s;
        foreach (var ch in _subscribers.Values) ch.Writer.TryWrite(new ClientEvent.ConnectionStateChanged(s));
    }

    // Caller holds _gate.
    private void DropSession()
    {
        _session?.End();
        _session = null;
        SetState(ConnectionState.Disconnected);
    }

    public void UseTransport(ITransport transport)
    {
        lock (_gate) _transport = (Transport)transport;
    }

    public async Task ConnectAsync(CancellationToken cancellationToken)
    {
        await _lifecycle.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            Transport link;
            lock (_gate)
            {
                if (_closed) throw Fail(new ClientError.Disconnected());
                if (_session is { Alive: true }) return; // already connected: idempotent
                SetState(ConnectionState.Connecting);
                link = _transport ?? new Transport(options.Endpoint);
            }
            Session session;
            try
            {
                session = await Session.OpenAsync(options, link, Publish, cancellationToken).ConfigureAwait(false);
            }
            catch
            {
                lock (_gate) SetState(ConnectionState.Disconnected);
                throw;
            }
            lock (_gate)
            {
                if (_closed)
                {
                    session.End();
                    SetState(ConnectionState.Disconnected);
                    throw Fail(new ClientError.Disconnected());
                }
                _session = session;
                SetState(ConnectionState.Connected);
            }
            // Report a session ended by the backend or the network.
            session.Ended.Register(() =>
            {
                lock (_gate)
                {
                    if (_session != session) return;
                    _session = null;
                    SetState(ConnectionState.Disconnected);
                }
            });
        }
        finally
        {
            _lifecycle.Release();
        }
    }

    public async Task DisconnectAsync(CancellationToken cancellationToken)
    {
        await _lifecycle.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            lock (_gate) DropSession();
        }
        finally
        {
            _lifecycle.Release();
        }
    }

    private Task<JsonElement> CallAsync(HttpMethod method, string path, Action<Utf8JsonWriter>? body, CancellationToken ct)
    {
        Session? session;
        lock (_gate) session = _session;
        if (session is not { Alive: true }) throw Fail(new ClientError.Disconnected());
        return session.CallAsync(method, path, body, options.RequestTimeoutMs, ct);
    }

    private static Customer ToCustomer(JsonElement json) => new(
        json.GetProperty("id").GetString()!,
        json.GetProperty("name").GetString()!,
        json.TryGetProperty("email", out var email) ? email.GetString() : null);

    public async Task<string> KeyFingerprintAsync(CancellationToken cancellationToken) =>
        (await CallAsync(HttpMethod.Get, Session.PathWhoami, null, cancellationToken).ConfigureAwait(false)).GetProperty("key").GetString()!;

    public async Task<Customer> GetCustomerAsync(string id, CancellationToken cancellationToken) =>
        ToCustomer(await CallAsync(HttpMethod.Get, $"{Session.PathCustomers}?id={Session.QueryValue(id)}", null, cancellationToken).ConfigureAwait(false));

    public async Task<Customer> CreateCustomerAsync(string name, string? email, CancellationToken cancellationToken) =>
        ToCustomer(await CallAsync(HttpMethod.Post, Session.PathCustomers, w =>
        {
            w.WriteString("name", name);
            if (email is null) w.WriteNull("email"); else w.WriteString("email", email);
        }, cancellationToken).ConfigureAwait(false));

    public async Task<CommandResult> SendCommandAsync(Command command, CancellationToken cancellationToken)
    {
        var json = await CallAsync(HttpMethod.Post, Session.PathCommands, w =>
        {
            w.WriteString("name", command.Name);
            w.WriteBase64String("payload", command.Payload);
        }, cancellationToken).ConfigureAwait(false);
        return new CommandResult(json.GetProperty("name").GetString()!, json.GetProperty("payload").GetBytesFromBase64());
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
            DropSession();
            _closed = true;
            foreach (var ch in _subscribers.Values) ch.Writer.TryComplete();
            _subscribers.Clear();
        }
    }
}
