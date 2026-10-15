using System.Reflection;
using Microsoft.UI.Dispatching;
using Ring.Api;

namespace Ring.Services;

public enum ConnectionStatus
{
    SignedOut,
    Connecting,
    Online,
    Offline,
}

/// <summary>
/// The app's composition root: the session, the API client, the realtime socket and the services
/// built on them. Everything here is touched on the UI thread; background events are marshalled.
/// </summary>
public sealed class AppServices
{
    public const string AppUserModelId = "TeamOfSilicons.Ring";
    public const string ProductionApiUrl = "https://backend.ring.teamofsilicons.com";
    public const string SourceUrl = "https://github.com/teamofsilicons/silicon-ring";

    private RealtimeConnection? _realtime;
    private StoredSession? _session;
    private ConnectionStatus _connection = ConnectionStatus.SignedOut;
    private bool _signingOut;
    private TimeSpan _clockOffset;

    public AppServices(DispatcherQueue dispatcher)
    {
        Dispatcher = dispatcher;
        Preferences = Preferences.Load();
        Api = CreateApi(ResolveApiUrl(null));
        Calls = new CallCoordinator(this);
    }

    public static string AppVersion
    {
        get
        {
            var version = Assembly.GetExecutingAssembly().GetName().Version;
            return version is null ? "1.0.0" : $"{version.Major}.{version.Minor}.{version.Build}";
        }
    }

    public static string UserAgent => $"Ring-Windows/{AppVersion}";

    public DispatcherQueue Dispatcher { get; }

    public Preferences Preferences { get; }

    public Sounds Sounds { get; } = new();

    public CallNotifications Notifications { get; } = new();

    public CallCoordinator Calls { get; }

    public RingApiClient Api { get; private set; }

    public Meta? Meta { get; private set; }

    public Account? Me => _session?.Account;

    public string? DeviceId => _session?.DeviceId;

    public bool IsSignedIn => _session is not null;

    /// <summary>"Ring on this device": when off, calls do not ring here.</summary>
    public bool RingEnabled { get; private set; } = true;

    public string? DeviceName { get; private set; }

    public ConnectionStatus Connection => _connection;

    /// <summary>
    /// The server's clock, estimated from realtime frames. Ringing expiry and call timers use it,
    /// so a PC clock that is off by a minute neither drops incoming calls nor skews the timer.
    /// </summary>
    public DateTimeOffset ServerNow => DateTimeOffset.UtcNow + _clockOffset;

    public event Action? SessionChanged;

    public event Action? ConnectionChanged;

    /// <summary>The current account or this device changed (name, ring setting).</summary>
    public event Action? ProfileChanged;

    public event Action? VoicemailsChanged;

    public event Action? DevicesChanged;

    /// <summary>A message for the user: (text, isError).</summary>
    public event Action<string, bool>? Notice;

    public void Post(Action action) => Dispatcher.TryEnqueue(() => action());

    public void ShowNotice(string message, bool isError = true) => Notice?.Invoke(message, isError);

    /// <summary>Local changes (read, deleted) refresh the lists and badge without waiting for the server.</summary>
    public void NotifyVoicemailsChanged() => VoicemailsChanged?.Invoke();

    /// <summary>
    /// The API origin: a session stays bound to the server that issued it; otherwise the server
    /// chosen on the sign-in screen, then RING_API_URL (development), then production.
    /// </summary>
    public string ResolveApiUrl(string? sessionUrl)
    {
        if (!string.IsNullOrWhiteSpace(sessionUrl))
        {
            return sessionUrl;
        }
        if (!string.IsNullOrWhiteSpace(Preferences.ApiUrl))
        {
            return Preferences.ApiUrl!.Trim();
        }
        var env = Environment.GetEnvironmentVariable("RING_API_URL");
        return string.IsNullOrWhiteSpace(env) ? ProductionApiUrl : env.Trim();
    }

    private RingApiClient CreateApi(string apiUrl)
    {
        if (!Uri.TryCreate(apiUrl.Trim().TrimEnd('/') + "/", UriKind.Absolute, out var baseUri) ||
            (baseUri.Scheme != Uri.UriSchemeHttps && baseUri.Scheme != Uri.UriSchemeHttp))
        {
            baseUri = new Uri(ProductionApiUrl + "/");
        }
        var api = new RingApiClient(baseUri, UserAgent);
        api.Unauthorized += (_, _) => Post(() => _ = SignOutLocallyAsync("Your session ended. Sign in again."));
        return api;
    }

    private void SetConnection(ConnectionStatus status)
    {
        if (_connection != status)
        {
            _connection = status;
            ConnectionChanged?.Invoke();
        }
    }

    // ---- session ----------------------------------------------------------------------------------

    /// <summary>Restores the saved session, if any, and starts listening for calls.</summary>
    public void Restore()
    {
        var stored = SessionStore.Load();
        if (stored is null)
        {
            SetConnection(ConnectionStatus.SignedOut);
            return;
        }
        Activate(stored);
        _ = RefreshProfileAsync(registerDevice: false);
    }

    /// <summary>The server's change, for development. Only allowed while signed out.</summary>
    public void UseServer(string? apiUrl)
    {
        if (IsSignedIn)
        {
            return;
        }
        Preferences.ApiUrl = string.IsNullOrWhiteSpace(apiUrl) || apiUrl.Trim() == ProductionApiUrl ? null : apiUrl.Trim();
        Preferences.Save();
        Api.Dispose();
        Api = CreateApi(ResolveApiUrl(null));
        Meta = null;
    }

    public async Task<Meta?> LoadMetaAsync()
    {
        try
        {
            Meta = await Api.GetMetaAsync();
        }
        catch (ApiException)
        {
            // Offline or an older server; sign-in falls back to the production Accounts URL.
        }
        return Meta;
    }

    public async Task SignInAsync(CancellationToken ct)
    {
        var meta = Meta ?? await LoadMetaAsync();
        var accountsUrl = meta?.AccountsUrl ?? "https://accounts.teamofsilicons.com";
        var grant = await SignInFlow.RunAsync(Api, accountsUrl, Environment.MachineName, ct);

        var stored = new StoredSession
        {
            ApiUrl = Api.BaseUri.ToString().TrimEnd('/'),
            SessionToken = grant.SessionToken,
            ExpiresAt = grant.ExpiresAt,
            Account = grant.Account,
            DeviceId = grant.DeviceId,
        };
        SessionStore.Save(stored);

        if (!Preferences.OpenAtLoginInitialized)
        {
            // Ring listens for calls from login onwards, like a phone; Settings can turn it off.
            try
            {
                StartupRegistration.SetEnabled(true);
            }
            catch (Exception)
            {
                // Group policy may block the Run key; the setting simply stays off.
            }
            Preferences.OpenAtLoginInitialized = true;
            Preferences.Save();
        }

        Activate(stored);
        await RefreshProfileAsync(registerDevice: true);
    }

    private void Activate(StoredSession stored)
    {
        _session = stored;
        _signingOut = false;
        if (Api.BaseUri.ToString().TrimEnd('/') != stored.ApiUrl.TrimEnd('/'))
        {
            Api.Dispose();
            Api = CreateApi(stored.ApiUrl);
        }
        Api.SessionToken = stored.SessionToken;
        SessionChanged?.Invoke();
        StartRealtime();
    }

    /// <summary>Reads /v1/me and tells the server what this device is.</summary>
    public async Task RefreshProfileAsync(bool registerDevice)
    {
        if (_session is null)
        {
            return;
        }
        try
        {
            var me = await Api.GetMeAsync();
            ApplyMe(me.Account, me.Device);
            var update = new DeviceUpdate
            {
                Name = registerDevice ? Environment.MachineName : null,
                Platform = "windows",
                AppVersion = AppVersion,
                // Windows has no push channel; the persistent socket is the wake path.
                Push = registerDevice ? new List<PushRegistration>() : null,
            };
            var device = await Api.UpdateCurrentDeviceAsync(update);
            ApplyMe(me.Account, device);
        }
        catch (ApiException)
        {
            // Offline: the cached session keeps working and the socket retries. A 401 has
            // already started the sign-out through RingApiClient.Unauthorized.
        }
    }

    private void ApplyMe(Account account, Device device)
    {
        if (_session is null)
        {
            return;
        }
        _session.Account = account;
        RingEnabled = device.RingEnabled;
        DeviceName = device.Name;
        try
        {
            SessionStore.Save(_session);
        }
        catch (Exception)
        {
            // The cached profile is a convenience.
        }
        ProfileChanged?.Invoke();
    }

    public async Task SetRingEnabledAsync(bool enabled)
    {
        var device = await Api.UpdateCurrentDeviceAsync(new DeviceUpdate { RingEnabled = enabled });
        if (Me is { } me)
        {
            ApplyMe(me, device);
        }
    }

    public async Task RenameDeviceAsync(string name)
    {
        var device = await Api.UpdateCurrentDeviceAsync(new DeviceUpdate { Name = name });
        if (Me is { } me)
        {
            ApplyMe(me, device);
        }
    }

    /// <summary>Signs out on the server too (revoking this device), then locally.</summary>
    public async Task SignOutAsync()
    {
        try
        {
            await Api.SignOutAsync();
        }
        catch (ApiException)
        {
            // Already invalid or offline; forget it locally either way.
        }
        await SignOutLocallyAsync(null);
    }

    private async Task SignOutLocallyAsync(string? reason)
    {
        if (_signingOut || _session is null)
        {
            return;
        }
        _signingOut = true;
        await Calls.ResetAsync();
        await StopRealtimeAsync();
        SessionStore.Clear();
        _session = null;
        Api.SessionToken = null;
        RingEnabled = true;
        DeviceName = null;
        SetConnection(ConnectionStatus.SignedOut);
        SessionChanged?.Invoke();
        if (reason is not null)
        {
            ShowNotice(reason, isError: false);
        }
    }

    // ---- realtime ---------------------------------------------------------------------------------

    private void StartRealtime()
    {
        if (_session is null)
        {
            return;
        }
        _ = StopRealtimeAsync();
        var realtime = new RealtimeConnection(Api.BaseUri, _session.SessionToken);
        realtime.Connected += () => Post(() =>
        {
            if (_realtime != realtime)
            {
                return;
            }
            SetConnection(ConnectionStatus.Online);
            _ = Calls.ReconcileAsync();
            _ = RefreshProfileAsync(registerDevice: false);
        });
        realtime.Disconnected += (_, _) => Post(() =>
        {
            if (_realtime == realtime)
            {
                SetConnection(ConnectionStatus.Offline);
            }
        });
        realtime.SignedOut += () => Post(() =>
        {
            if (_realtime == realtime)
            {
                _ = SignOutLocallyAsync("This device was signed out of Ring.");
            }
        });
        realtime.EventReceived += frame =>
        {
            var received = DateTimeOffset.UtcNow;
            Post(() =>
            {
                if (_realtime != realtime)
                {
                    return;
                }
                if (frame.At is { } at)
                {
                    _clockOffset = at - received;
                }
                HandleEvent(frame);
            });
        };
        _realtime = realtime;
        SetConnection(ConnectionStatus.Connecting);
        realtime.Start();
    }

    private async Task StopRealtimeAsync()
    {
        var realtime = _realtime;
        _realtime = null;
        if (realtime is not null)
        {
            await realtime.DisposeAsync();
        }
    }

    /// <summary>After sleep or a network change the old socket may be silently dead.</summary>
    public void ReconnectSoon() => _realtime?.Kick();

    private void HandleEvent(RealtimeEvent frame)
    {
        try
        {
            switch (frame.Type)
            {
                case EventTypes.VoicemailReceived:
                    var received = frame.DataAs<VoicemailData>().Voicemail;
                    if (received.To.Uuid == Me?.Uuid)
                    {
                        Notifications.ShowVoicemail(received);
                    }
                    VoicemailsChanged?.Invoke();
                    break;
                case EventTypes.VoicemailUpdated:
                    VoicemailsChanged?.Invoke();
                    break;
                case EventTypes.DeviceUpdated:
                    var device = frame.DataAs<DeviceData>().Device;
                    if (device.DeviceId == DeviceId && Me is { } me)
                    {
                        ApplyMe(me, device);
                    }
                    DevicesChanged?.Invoke();
                    break;
                default:
                    Calls.HandleEvent(frame);
                    break;
            }
        }
        catch (Exception ex) when (ex is System.Text.Json.JsonException or ApiException or InvalidOperationException or NotSupportedException)
        {
            // A frame this version cannot read; the next reconcile catches up.
            LocalLog.Write($"skipped a {frame.Type} event: {ex.Message}");
        }
    }

    public async Task ShutdownAsync()
    {
        await Calls.ResetAsync();
        await StopRealtimeAsync();
        Notifications.Unregister();
        Sounds.Dispose();
    }
}
