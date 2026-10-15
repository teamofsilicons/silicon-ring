using System.Net;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Ring.Api;

/// <summary>A non-2xx answer from the Ring API, or a transport failure (<see cref="Status"/> 0).</summary>
public sealed class ApiException : Exception
{
    public ApiException(int status, string code, string message, string? hint = null, bool retryable = false, JsonElement details = default)
        : base(message)
    {
        Status = status;
        Code = code;
        Hint = hint;
        Retryable = retryable;
        Details = details;
    }

    public int Status { get; }
    public string Code { get; }
    public string? Hint { get; }
    public bool Retryable { get; }
    public JsonElement Details { get; }

    public bool IsUnauthorized => Status == 401;

    /// <summary>A sentence for the user: the server's message plus its hint.</summary>
    public string UserMessage => string.IsNullOrWhiteSpace(Hint) ? Message : $"{Message} {Hint}";
}

/// <summary>Filters for <c>GET /v1/calls</c>.</summary>
public sealed record CallQuery(string? States = null, string? Direction = null, string? With = null, int? Limit = null, string? Cursor = null);

/// <summary>Every HTTP endpoint of udd/api.md that a carbon's desktop app uses.</summary>
public sealed class RingApiClient : IDisposable
{
    private readonly HttpClient _http;

    public RingApiClient(Uri baseUri, string userAgent)
    {
        BaseUri = baseUri;
        var handler = new SocketsHttpHandler
        {
            PooledConnectionLifetime = TimeSpan.FromMinutes(5),
            ConnectTimeout = TimeSpan.FromSeconds(10),
            AutomaticDecompression = DecompressionMethods.All,
        };
        _http = new HttpClient(handler) { BaseAddress = baseUri, Timeout = TimeSpan.FromSeconds(25) };
        _http.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/json"));
        _http.DefaultRequestHeaders.UserAgent.ParseAdd(userAgent);
    }

    public Uri BaseUri { get; }

    /// <summary>The Ring session (<c>rst_…</c>) sent as a Bearer token, or null when signed out.</summary>
    public string? SessionToken { get; set; }

    /// <summary>Raised (on the calling thread) when a request is refused with 401.</summary>
    public event EventHandler? Unauthorized;

    public void Dispose() => _http.Dispose();

    // ---- public ---------------------------------------------------------------------------------

    public Task<Meta> GetMetaAsync(CancellationToken ct = default) => SendAsync<Meta>(HttpMethod.Get, "v1/meta", ct: ct);

    // ---- auth -----------------------------------------------------------------------------------

    public Task<SessionGrant> SignInCarbonAsync(CarbonLoginRequest request, CancellationToken ct = default) =>
        SendAsync<SessionGrant>(HttpMethod.Post, "v1/auth/carbon", request, ct: ct);

    public Task<MeResponse> GetMeAsync(CancellationToken ct = default) => SendAsync<MeResponse>(HttpMethod.Get, "v1/me", ct: ct);

    public Task SignOutAsync(CancellationToken ct = default) => SendAsync(HttpMethod.Delete, "v1/session", ct: ct);

    // ---- devices --------------------------------------------------------------------------------

    public Task<Device> UpdateCurrentDeviceAsync(DeviceUpdate update, CancellationToken ct = default) =>
        SendAsync<Device>(HttpMethod.Put, "v1/devices/current", update, ct: ct);

    public async Task<List<Device>> GetDevicesAsync(CancellationToken ct = default) =>
        (await SendAsync<ItemsResponse<Device>>(HttpMethod.Get, "v1/devices", ct: ct)).Items;

    public Task<Device> PatchDeviceAsync(string deviceId, DevicePatch patch, CancellationToken ct = default) =>
        SendAsync<Device>(HttpMethod.Patch, $"v1/devices/{Seg(deviceId)}", patch, ct: ct);

    public Task RevokeDeviceAsync(string deviceId, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Delete, $"v1/devices/{Seg(deviceId)}", ct: ct);

    // ---- people ---------------------------------------------------------------------------------

    public Task<AccountLookup> LookupAccountAsync(string reference, CancellationToken ct = default) =>
        SendAsync<AccountLookup>(HttpMethod.Get, $"v1/accounts/{Seg(reference)}", ct: ct);

    public async Task<List<RecentPerson>> GetRecentPeopleAsync(string? query = null, int limit = 20, CancellationToken ct = default)
    {
        var path = $"v1/people/recent?limit={limit}";
        if (!string.IsNullOrWhiteSpace(query))
        {
            path += $"&q={Uri.EscapeDataString(query)}";
        }
        return (await SendAsync<ItemsResponse<RecentPerson>>(HttpMethod.Get, path, ct: ct)).Items;
    }

    // ---- calls ----------------------------------------------------------------------------------

    public Task<CreatedCall> CreateCallAsync(string to, CancellationToken ct = default) =>
        SendAsync<CreatedCall>(HttpMethod.Post, "v1/calls", new CreateCallRequest { To = to }, NewIdempotencyKey(), ct);

    public Task<ApiPage<Call>> ListCallsAsync(CallQuery query, CancellationToken ct = default)
    {
        var parts = new List<string>();
        if (query.States is not null) parts.Add($"state={Uri.EscapeDataString(query.States)}");
        if (query.Direction is not null) parts.Add($"direction={Uri.EscapeDataString(query.Direction)}");
        if (query.With is not null) parts.Add($"with={Uri.EscapeDataString(query.With)}");
        if (query.Limit is int limit) parts.Add($"limit={limit}");
        if (query.Cursor is not null) parts.Add($"cursor={Uri.EscapeDataString(query.Cursor)}");
        var path = parts.Count == 0 ? "v1/calls" : "v1/calls?" + string.Join('&', parts);
        return SendAsync<ApiPage<Call>>(HttpMethod.Get, path, ct: ct);
    }

    public Task<Call> GetCallAsync(string ringid, CancellationToken ct = default) =>
        SendAsync<Call>(HttpMethod.Get, $"v1/calls/{Seg(ringid)}", ct: ct);

    public Task<CallWithMedia> AcceptAsync(string ringid, string? invitationId, CancellationToken ct = default) =>
        SendAsync<CallWithMedia>(HttpMethod.Post, $"v1/calls/{Seg(ringid)}/accept", new AcceptRequest { InvitationId = invitationId }, ct: ct);

    public async Task<Call> DeclineAsync(string ringid, string? invitationId, string? reason, CancellationToken ct = default)
    {
        var body = new DeclineRequest { InvitationId = invitationId, Reason = string.IsNullOrWhiteSpace(reason) ? null : reason.Trim() };
        return (await SendAsync<CallEnvelope>(HttpMethod.Post, $"v1/calls/{Seg(ringid)}/decline", body, ct: ct)).Call;
    }

    public async Task<Call> SilenceAsync(string ringid, string? invitationId, CancellationToken ct = default) =>
        (await SendAsync<CallEnvelope>(HttpMethod.Post, $"v1/calls/{Seg(ringid)}/silence", new SilenceRequest { InvitationId = invitationId }, ct: ct)).Call;

    public async Task<Call> LeaveAsync(string ringid, CancellationToken ct = default) =>
        (await SendAsync<CallEnvelope>(HttpMethod.Post, $"v1/calls/{Seg(ringid)}/leave", new JsonObject(), ct: ct)).Call;

    public Task<InvitedCall> InviteAsync(string ringid, string to, CancellationToken ct = default) =>
        SendAsync<InvitedCall>(HttpMethod.Post, $"v1/calls/{Seg(ringid)}/invite", new InviteRequest { To = to }, NewIdempotencyKey(), ct);

    /// <summary>A media grant for this device: rejoin, or hand the call off to this device.</summary>
    public Task<CallWithMedia> ClaimMediaAsync(string ringid, CancellationToken ct = default) =>
        SendAsync<CallWithMedia>(HttpMethod.Post, $"v1/calls/{Seg(ringid)}/media", new JsonObject(), ct: ct);

    public Task<TranscriptPage> GetTranscriptAsync(string ringid, long afterSeq = 0, int limit = 200, CancellationToken ct = default) =>
        SendAsync<TranscriptPage>(HttpMethod.Get, $"v1/calls/{Seg(ringid)}/transcript?after_seq={afterSeq}&limit={limit}", ct: ct);

    public Task<Recording> GetRecordingAsync(string ringid, CancellationToken ct = default) =>
        SendAsync<Recording>(HttpMethod.Get, $"v1/calls/{Seg(ringid)}/recording", ct: ct);

    // ---- voicemail ------------------------------------------------------------------------------

    public Task<ApiPage<Voicemail>> ListVoicemailsAsync(bool unreadOnly = false, int limit = 50, string? cursor = null, CancellationToken ct = default)
    {
        var path = $"v1/voicemails?limit={limit}";
        if (unreadOnly) path += "&unread=true";
        if (cursor is not null) path += $"&cursor={Uri.EscapeDataString(cursor)}";
        return SendAsync<ApiPage<Voicemail>>(HttpMethod.Get, path, ct: ct);
    }

    public Task<Voicemail> GetVoicemailAsync(string id, CancellationToken ct = default) =>
        SendAsync<Voicemail>(HttpMethod.Get, $"v1/voicemails/{Seg(id)}", ct: ct);

    public Task<Voicemail> SetVoicemailReadAsync(string id, bool read, CancellationToken ct = default) =>
        SendAsync<Voicemail>(HttpMethod.Patch, $"v1/voicemails/{Seg(id)}", new VoicemailReadPatch { Read = read }, ct: ct);

    public Task DeleteVoicemailAsync(string id, CancellationToken ct = default) =>
        SendAsync(HttpMethod.Delete, $"v1/voicemails/{Seg(id)}", ct: ct);

    // ---- settings -------------------------------------------------------------------------------

    public Task<RingSettings> GetSettingsAsync(CancellationToken ct = default) =>
        SendAsync<RingSettings>(HttpMethod.Get, "v1/settings", ct: ct);

    public Task<RingSettings> PatchSettingsAsync(JsonObject patch, CancellationToken ct = default) =>
        SendAsync<RingSettings>(HttpMethod.Patch, "v1/settings", patch, ct: ct);

    public Task<VoiceList> GetVoicesAsync(CancellationToken ct = default) =>
        SendAsync<VoiceList>(HttpMethod.Get, "v1/voices", ct: ct);

    // ---- plumbing -------------------------------------------------------------------------------

    private static string Seg(string value) => Uri.EscapeDataString(value);

    private static string NewIdempotencyKey() => "win-" + Guid.NewGuid().ToString("N");

    private async Task<T> SendAsync<T>(HttpMethod method, string path, object? body = null, string? idempotencyKey = null, CancellationToken ct = default)
    {
        using var response = await SendCoreAsync(method, path, body, idempotencyKey, ct).ConfigureAwait(false);
        try
        {
            var value = await response.Content.ReadFromJsonAsync<T>(RingJson.Options, ct).ConfigureAwait(false);
            return value ?? throw new ApiException((int)response.StatusCode, "protocol", "The server sent an empty answer.");
        }
        catch (JsonException ex)
        {
            throw new ApiException((int)response.StatusCode, "protocol", $"The server's answer could not be read ({ex.Message}).");
        }
    }

    private async Task SendAsync(HttpMethod method, string path, object? body = null, string? idempotencyKey = null, CancellationToken ct = default)
    {
        using var response = await SendCoreAsync(method, path, body, idempotencyKey, ct).ConfigureAwait(false);
    }

    private async Task<HttpResponseMessage> SendCoreAsync(HttpMethod method, string path, object? body, string? idempotencyKey, CancellationToken ct)
    {
        using var request = new HttpRequestMessage(method, path);
        if (SessionToken is { Length: > 0 } token)
        {
            request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token);
        }
        if (idempotencyKey is not null)
        {
            request.Headers.TryAddWithoutValidation("Idempotency-Key", idempotencyKey);
        }
        if (body is not null)
        {
            var json = body is JsonNode node ? node.ToJsonString(RingJson.Options) : JsonSerializer.Serialize(body, body.GetType(), RingJson.Options);
            request.Content = new StringContent(json, Encoding.UTF8, "application/json");
        }

        HttpResponseMessage response;
        try
        {
            response = await _http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, ct).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or IOException)
        {
            throw new ApiException(0, "network", "Ring can't reach the server. Check your connection.", retryable: true);
        }

        if (response.IsSuccessStatusCode)
        {
            return response;
        }

        using (response)
        {
            var status = (int)response.StatusCode;
            ApiErrorInfo? error = null;
            try
            {
                error = (await response.Content.ReadFromJsonAsync<ApiErrorBody>(RingJson.Options, ct).ConfigureAwait(false))?.Error;
            }
            catch (Exception ex) when (ex is JsonException or NotSupportedException or HttpRequestException)
            {
                // Not a Ring error body (a proxy page, for example); report the status alone.
            }
            if (status == 401)
            {
                Unauthorized?.Invoke(this, EventArgs.Empty);
            }
            throw error is null
                ? new ApiException(status, status == 401 ? "unauthenticated" : "http_" + status, $"The server answered {status} {response.ReasonPhrase}.", retryable: status >= 500)
                : new ApiException(status, error.Code, error.Message, error.Hint, error.Retryable, error.Details);
        }
    }
}
