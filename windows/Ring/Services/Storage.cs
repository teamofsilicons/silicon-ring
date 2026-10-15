using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Ring.Api;

namespace Ring.Services;

/// <summary>%LOCALAPPDATA%\Ring, where the app keeps its session and preferences.</summary>
public static class AppPaths
{
    public static string DataDirectory
    {
        get
        {
            var dir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Ring");
            Directory.CreateDirectory(dir);
            return dir;
        }
    }

    public static string Asset(params string[] parts) => Path.Combine(AppContext.BaseDirectory, "Assets", Path.Combine(parts));
}

/// <summary>The signed-in session as stored on disk.</summary>
public sealed class StoredSession
{
    public string ApiUrl { get; set; } = "";
    public string SessionToken { get; set; } = "";
    public DateTimeOffset ExpiresAt { get; set; }
    public Account Account { get; set; } = new();
    public string DeviceId { get; set; } = "";
}

/// <summary>
/// Keeps the Ring session token encrypted with DPAPI for the current Windows user, so only this
/// user on this machine can read it.
/// </summary>
public static class SessionStore
{
    private static readonly byte[] Entropy = Encoding.UTF8.GetBytes("TeamOfSilicons.Ring.session.v1");

    private static string FilePath => Path.Combine(AppPaths.DataDirectory, "session.dat");

    public static StoredSession? Load()
    {
        try
        {
            if (!File.Exists(FilePath))
            {
                return null;
            }
            var plain = ProtectedData.Unprotect(File.ReadAllBytes(FilePath), Entropy, DataProtectionScope.CurrentUser);
            var session = JsonSerializer.Deserialize<StoredSession>(plain, RingJson.Options);
            return session is { SessionToken.Length: > 0, ApiUrl.Length: > 0 } ? session : null;
        }
        catch (Exception ex) when (ex is CryptographicException or JsonException or IOException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    public static void Save(StoredSession session)
    {
        var plain = JsonSerializer.SerializeToUtf8Bytes(session, RingJson.Options);
        var sealedBytes = ProtectedData.Protect(plain, Entropy, DataProtectionScope.CurrentUser);
        CryptographicOperations.ZeroMemory(plain);
        var temp = FilePath + ".tmp";
        File.WriteAllBytes(temp, sealedBytes);
        File.Move(temp, FilePath, overwrite: true);
    }

    public static void Clear()
    {
        try
        {
            File.Delete(FilePath);
        }
        catch (IOException)
        {
            // Nothing to remove.
        }
    }
}

/// <summary>Per-user app preferences (not secret), as JSON.</summary>
public sealed class Preferences
{
    /// <summary><c>system</c>, <c>light</c> or <c>dark</c>.</summary>
    public string Theme { get; set; } = "system";

    public bool KeepRunningInTray { get; set; } = true;

    /// <summary>Set once the app has applied its default of opening at login.</summary>
    public bool OpenAtLoginInitialized { get; set; }

    public string? InputDeviceId { get; set; }

    public string? OutputDeviceId { get; set; }

    /// <summary>Overrides the API origin (development servers). Null means production.</summary>
    public string? ApiUrl { get; set; }

    private static string FilePath => Path.Combine(AppPaths.DataDirectory, "preferences.json");

    public static Preferences Load()
    {
        try
        {
            if (File.Exists(FilePath))
            {
                return JsonSerializer.Deserialize<Preferences>(File.ReadAllText(FilePath), RingJson.Options) ?? new Preferences();
            }
        }
        catch (Exception ex) when (ex is JsonException or IOException or UnauthorizedAccessException)
        {
            // A damaged file falls back to the defaults.
        }
        return new Preferences();
    }

    public void Save()
    {
        try
        {
            var temp = FilePath + ".tmp";
            File.WriteAllText(temp, JsonSerializer.Serialize(this, RingJson.Options));
            File.Move(temp, FilePath, overwrite: true);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            // Preferences are a convenience; failing to save them must not break the app.
        }
    }
}
