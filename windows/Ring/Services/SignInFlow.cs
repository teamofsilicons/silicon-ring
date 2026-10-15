using System.Collections.Specialized;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Web;
using Ring.Api;

namespace Ring.Services;

/// <summary>
/// Carbon sign-in for a desktop app (RFC 8252): the system browser opens Silicon Accounts with
/// PKCE, and a loopback listener on 127.0.0.1 receives the redirect. See udd/accounts.md.
/// </summary>
public static class SignInFlow
{
    private const string CallbackPath = "/auth/callback";

    private const string PageTemplate = """
        <!doctype html>
        <html lang="en"><head><meta charset="utf-8"><title>Ring</title>
        <style>
          body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;
               background:#FFFDF9;color:#1E1E1C;font:16px/1.5 system-ui,"Segoe UI",sans-serif}
          main{text-align:center;padding:24px}
          h1{font:400 26px/1.2 Georgia,serif;margin:0 0 8px}
          p{color:#6E6B63;margin:0}
        </style></head>
        <body><main><h1>{TITLE}</h1><p>{BODY}</p></main></body></html>
        """;

    /// <summary>What one request to the loopback address meant.</summary>
    private sealed record Reply(string? Code, string Title, string Body);

    /// <summary>
    /// Runs the whole flow and returns the Ring session. Throws <see cref="OperationCanceledException"/>
    /// when the user cancels in the browser or <paramref name="ct"/> fires.
    /// </summary>
    public static async Task<SessionGrant> RunAsync(RingApiClient api, string accountsUrl, string deviceLabel, CancellationToken ct)
    {
        var verifier = RandomUrlSafe(48);
        var challenge = Base64Url(SHA256.HashData(Encoding.ASCII.GetBytes(verifier)));
        var state = RandomUrlSafe(24);

        var port = FreeLoopbackPort();
        var redirectUri = $"http://127.0.0.1:{port}{CallbackPath}";

        // HttpListener (http.sys) normally accepts a loopback prefix without elevation; if policy
        // refuses it, a plain socket on the same port does the job.
        HttpListener? http = new();
        TcpListener? tcp = null;
        try
        {
            http.Prefixes.Add($"http://127.0.0.1:{port}/");
            http.Start();
        }
        catch (HttpListenerException)
        {
            http.Close();
            http = null;
            tcp = new TcpListener(IPAddress.Loopback, port);
            tcp.Start();
        }

        try
        {
            var authorize = $"{accountsUrl.TrimEnd('/')}/authorize?response_type=code&app_id=ring" +
                            $"&redirect_uri={Uri.EscapeDataString(redirectUri)}&scope=profile" +
                            $"&state={Uri.EscapeDataString(state)}&code_challenge={Uri.EscapeDataString(challenge)}" +
                            "&code_challenge_method=S256";
            Process.Start(new ProcessStartInfo(authorize) { UseShellExecute = true });

            var code = http is not null
                ? await WaitWithHttpListenerAsync(http, state, ct).ConfigureAwait(false)
                : await WaitWithSocketAsync(tcp!, state, ct).ConfigureAwait(false);

            // Each code is single-use and short-lived, so it is exchanged exactly once.
            return await api.SignInCarbonAsync(new CarbonLoginRequest
            {
                Code = code,
                CodeVerifier = verifier,
                RedirectUri = redirectUri,
                Label = deviceLabel,
            }, ct).ConfigureAwait(false);
        }
        finally
        {
            http?.Close();
            tcp?.Stop();
        }
    }

    /// <summary>Decides what a request to the loopback address means; throws when sign-in failed.</summary>
    private static Reply Interpret(string? path, NameValueCollection query, string state)
    {
        if (!string.Equals(path, CallbackPath, StringComparison.Ordinal))
        {
            return new Reply(null, "Not found", "This address only receives the Ring sign-in.");
        }
        var error = query["error"];
        if (!string.IsNullOrEmpty(error) && StateAllowsError(query, state))
        {
            return new Reply(null, error == "access_denied" ? "Sign-in cancelled" : "Sign-in failed",
                "You can close this window and return to Ring.");
        }
        if (!string.Equals(query["state"], state, StringComparison.Ordinal) || string.IsNullOrEmpty(query["code"]))
        {
            // A stray or forged request; keep waiting for the real redirect.
            return new Reply(null, "Not this one", "This sign-in link is not for this window.");
        }
        return new Reply(query["code"], "You're signed in", "You can close this window and return to Ring.");
    }

    /// <summary>
    /// An error redirect ends the sign-in only when it carries this attempt's state (or none, as
    /// some error pages omit it), so a stray local request can't cancel a sign-in in progress.
    /// </summary>
    private static bool StateAllowsError(NameValueCollection query, string state)
    {
        var returned = query["state"];
        return string.IsNullOrEmpty(returned) || string.Equals(returned, state, StringComparison.Ordinal);
    }

    /// <summary>Throws for an Accounts error redirect after the browser got its answer.</summary>
    private static void ThrowIfFailed(string? path, NameValueCollection query, string state)
    {
        var error = query["error"];
        if (!string.Equals(path, CallbackPath, StringComparison.Ordinal) || string.IsNullOrEmpty(error) ||
            !StateAllowsError(query, state))
        {
            return;
        }
        if (error == "access_denied")
        {
            throw new OperationCanceledException("Sign-in was cancelled.");
        }
        throw new ApiException(0, error, query["error_description"] ?? "Silicon Accounts could not sign you in.");
    }

    private static async Task<string> WaitWithHttpListenerAsync(HttpListener listener, string state, CancellationToken ct)
    {
        using var stop = ct.Register(listener.Stop);
        while (true)
        {
            HttpListenerContext context;
            try
            {
                context = await listener.GetContextAsync().ConfigureAwait(false);
            }
            catch (Exception) when (ct.IsCancellationRequested)
            {
                throw new OperationCanceledException(ct);
            }

            var path = context.Request.Url?.AbsolutePath;
            var query = HttpUtility.ParseQueryString(context.Request.Url?.Query ?? "");
            var reply = Interpret(path, query, state);
            var bytes = Page(reply);
            context.Response.StatusCode = path == CallbackPath ? 200 : 404;
            context.Response.ContentType = "text/html; charset=utf-8";
            context.Response.ContentLength64 = bytes.Length;
            context.Response.Headers["Cache-Control"] = "no-store";
            await context.Response.OutputStream.WriteAsync(bytes, ct).ConfigureAwait(false);
            context.Response.Close();

            ThrowIfFailed(path, query, state);
            if (reply.Code is not null)
            {
                return reply.Code;
            }
        }
    }

    private static async Task<string> WaitWithSocketAsync(TcpListener listener, string state, CancellationToken ct)
    {
        while (true)
        {
            using var client = await listener.AcceptTcpClientAsync(ct).ConfigureAwait(false);
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(ct);
            timeout.CancelAfter(TimeSpan.FromSeconds(10));
            var stream = client.GetStream();

            // Only the request line matters: "GET /auth/callback?code=…&state=… HTTP/1.1".
            var buffer = new byte[8192];
            var length = 0;
            try
            {
                while (length < buffer.Length)
                {
                    var read = await stream.ReadAsync(buffer.AsMemory(length), timeout.Token).ConfigureAwait(false);
                    if (read == 0)
                    {
                        break;
                    }
                    length += read;
                    if (Encoding.ASCII.GetString(buffer, 0, length).Contains("\r\n\r\n", StringComparison.Ordinal))
                    {
                        break;
                    }
                }
            }
            catch (OperationCanceledException) when (!ct.IsCancellationRequested)
            {
                continue;
            }

            var requestLine = Encoding.ASCII.GetString(buffer, 0, length).Split("\r\n")[0];
            var target = requestLine.Split(' ') is [_, var t, ..] ? t : "/";
            var question = target.IndexOf('?');
            var path = question < 0 ? target : target[..question];
            var query = HttpUtility.ParseQueryString(question < 0 ? "" : target[question..]);
            var reply = Interpret(path, query, state);
            var body = Page(reply);
            var head = Encoding.ASCII.GetBytes(
                $"HTTP/1.1 {(path == CallbackPath ? "200 OK" : "404 Not Found")}\r\n" +
                "Content-Type: text/html; charset=utf-8\r\n" +
                $"Content-Length: {body.Length}\r\n" +
                "Cache-Control: no-store\r\nConnection: close\r\n\r\n");
            await stream.WriteAsync(head, ct).ConfigureAwait(false);
            await stream.WriteAsync(body, ct).ConfigureAwait(false);

            ThrowIfFailed(path, query, state);
            if (reply.Code is not null)
            {
                return reply.Code;
            }
        }
    }

    private static byte[] Page(Reply reply) =>
        Encoding.UTF8.GetBytes(PageTemplate
            .Replace("{TITLE}", WebUtility.HtmlEncode(reply.Title))
            .Replace("{BODY}", WebUtility.HtmlEncode(reply.Body)));

    private static int FreeLoopbackPort()
    {
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        var port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        return port;
    }

    private static string RandomUrlSafe(int bytes) => Base64Url(RandomNumberGenerator.GetBytes(bytes));

    private static string Base64Url(byte[] data) =>
        Convert.ToBase64String(data).TrimEnd('=').Replace('+', '-').Replace('/', '_');
}
