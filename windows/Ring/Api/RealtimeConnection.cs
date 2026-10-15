using System.Net;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;

namespace Ring.Api;

/// <summary>
/// The realtime socket (<c>GET /v1/ws</c>), kept open for as long as the app runs: it sends
/// <c>{"type":"ping"}</c> every 25 s, treats 70 s of silence as a dead connection, and reconnects
/// with jittered backoff (1 s up to 30 s). Events are raised on a background thread.
/// </summary>
public sealed class RealtimeConnection : IAsyncDisposable
{
    private static readonly TimeSpan PingInterval = TimeSpan.FromSeconds(25);
    private static readonly TimeSpan StaleAfter = TimeSpan.FromSeconds(70);
    private static readonly ReadOnlyMemory<byte> PingFrame = Encoding.UTF8.GetBytes("{\"type\":\"ping\"}");

    private enum Outcome
    {
        Stopped,
        SignedOut,
        ConnectFailed,
        Dropped,
    }

    private readonly Uri _uri;
    private readonly string _token;
    private readonly CancellationTokenSource _stop = new();
    private Task? _loop;
    private volatile ClientWebSocket? _socket;

    public RealtimeConnection(Uri apiBase, string sessionToken)
    {
        var builder = new UriBuilder(new Uri(apiBase, "v1/ws"))
        {
            Scheme = apiBase.Scheme == Uri.UriSchemeHttps ? "wss" : "ws",
        };
        _uri = builder.Uri;
        _token = sessionToken;
    }

    /// <summary>Connected or reconnected. Events may have been missed; reconcile over REST.</summary>
    public event Action? Connected;

    public event Action<RealtimeEvent>? EventReceived;

    /// <summary>The connection dropped; the next attempt starts after the given delay.</summary>
    public event Action<string, TimeSpan>? Disconnected;

    /// <summary>The session is no longer valid (401 or <c>session.revoked</c>). No reconnects follow.</summary>
    public event Action? SignedOut;

    public bool IsConnected => _socket?.State == WebSocketState.Open;

    public void Start() => _loop ??= Task.Run(() => RunAsync(_stop.Token));

    /// <summary>Drops the current socket so the loop reconnects at once (after resume or a network change).</summary>
    public void Kick()
    {
        try
        {
            _socket?.Abort();
        }
        catch (Exception)
        {
            // The socket is already gone.
        }
    }

    public async ValueTask DisposeAsync()
    {
        _stop.Cancel();
        Kick();
        if (_loop is not null)
        {
            try
            {
                await _loop.ConfigureAwait(false);
            }
            catch (Exception)
            {
                // The loop only ends by cancellation.
            }
        }
        _stop.Dispose();
    }

    private async Task RunAsync(CancellationToken ct)
    {
        var attempt = 0;
        while (!ct.IsCancellationRequested)
        {
            var (outcome, reason) = await RunOnceAsync(ct).ConfigureAwait(false);
            switch (outcome)
            {
                case Outcome.Stopped:
                    return;
                case Outcome.SignedOut:
                    SignedOut?.Invoke();
                    return;
                case Outcome.Dropped:
                    attempt = 0;
                    break;
            }

            var delay = Backoff(attempt++);
            Disconnected?.Invoke(reason, delay);
            try
            {
                await Task.Delay(delay, ct).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return;
            }
        }
    }

    private static TimeSpan Backoff(int attempt)
    {
        var ceiling = Math.Min(30.0, Math.Pow(2, Math.Min(attempt, 5)));
        var jittered = ceiling * (0.5 + Random.Shared.NextDouble() * 0.5);
        return TimeSpan.FromSeconds(Math.Max(1.0, jittered));
    }

    private async Task<(Outcome, string)> RunOnceAsync(CancellationToken ct)
    {
        using var socket = new ClientWebSocket();
        socket.Options.SetRequestHeader("Authorization", "Bearer " + _token);
        socket.Options.CollectHttpResponseDetails = true;
        socket.Options.KeepAliveInterval = TimeSpan.FromSeconds(20);
        try
        {
            using var connectTimeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
            connectTimeout.CancelAfter(TimeSpan.FromSeconds(15));
            await socket.ConnectAsync(_uri, connectTimeout.Token).ConfigureAwait(false);
        }
        catch (Exception) when (ct.IsCancellationRequested)
        {
            return (Outcome.Stopped, "stopped");
        }
        catch (Exception ex)
        {
            if (socket.HttpStatusCode is HttpStatusCode.Unauthorized)
            {
                return (Outcome.SignedOut, "unauthenticated");
            }
            return (Outcome.ConnectFailed, ex.Message);
        }

        _socket = socket;
        try
        {
            Connected?.Invoke();
            return await PumpAsync(socket, ct).ConfigureAwait(false);
        }
        finally
        {
            _socket = null;
        }
    }

    /// <summary>Runs one connection until it fails or ends.</summary>
    private async Task<(Outcome, string)> PumpAsync(ClientWebSocket socket, CancellationToken ct)
    {
        using var connection = CancellationTokenSource.CreateLinkedTokenSource(ct);
        long lastHeard = Environment.TickCount64;

        var pinger = Task.Run(async () =>
        {
            try
            {
                while (!connection.IsCancellationRequested)
                {
                    await Task.Delay(PingInterval, connection.Token).ConfigureAwait(false);
                    if (Environment.TickCount64 - Interlocked.Read(ref lastHeard) > StaleAfter.TotalMilliseconds)
                    {
                        socket.Abort();
                        return;
                    }
                    await socket.SendAsync(PingFrame, WebSocketMessageType.Text, true, connection.Token).ConfigureAwait(false);
                }
            }
            catch (Exception)
            {
                // The receive loop notices the failure and ends the connection.
            }
        });

        var buffer = new byte[16 * 1024];
        using var message = new MemoryStream();
        try
        {
            while (true)
            {
                var result = await socket.ReceiveAsync(buffer.AsMemory(), connection.Token).ConfigureAwait(false);
                if (result.MessageType == WebSocketMessageType.Close)
                {
                    var status = socket.CloseStatus;
                    try
                    {
                        await socket.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, null, CancellationToken.None).ConfigureAwait(false);
                    }
                    catch (Exception)
                    {
                        // Closing is best effort.
                    }
                    return (Outcome.Dropped, $"closed by the server ({status})");
                }

                message.Write(buffer, 0, result.Count);
                if (!result.EndOfMessage)
                {
                    continue;
                }

                Interlocked.Exchange(ref lastHeard, Environment.TickCount64);
                var frame = Parse(message.GetBuffer().AsSpan(0, (int)message.Length));
                message.SetLength(0);
                if (frame is null || frame.Type == EventTypes.Pong)
                {
                    continue;
                }
                if (frame.Type == EventTypes.SessionRevoked)
                {
                    return (Outcome.SignedOut, "session revoked");
                }
                try
                {
                    EventReceived?.Invoke(frame);
                }
                catch (Exception)
                {
                    // A handler failure must not take the connection down.
                }
            }
        }
        catch (Exception) when (ct.IsCancellationRequested)
        {
            return (Outcome.Stopped, "stopped");
        }
        catch (Exception ex)
        {
            return (Outcome.Dropped, ex.Message);
        }
        finally
        {
            connection.Cancel();
            try
            {
                await pinger.ConfigureAwait(false);
            }
            catch (Exception)
            {
                // Already reported through the receive loop.
            }
        }
    }

    private static RealtimeEvent? Parse(ReadOnlySpan<byte> utf8)
    {
        try
        {
            return JsonSerializer.Deserialize<RealtimeEvent>(utf8, RingJson.Options);
        }
        catch (JsonException)
        {
            return null;
        }
    }
}
