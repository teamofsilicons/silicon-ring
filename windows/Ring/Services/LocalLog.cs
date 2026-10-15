namespace Ring.Services;

/// <summary>
/// A small local log at %LOCALAPPDATA%\Ring\ring.log for diagnosing failures on this PC. It never
/// leaves the machine and never records tokens or what was said on a call.
/// </summary>
public static class LocalLog
{
    private const long MaxBytes = 512 * 1024;
    private static readonly object Gate = new();

    private static string FilePath => Path.Combine(AppPaths.DataDirectory, "ring.log");

    public static void Error(string context, Exception exception) =>
        Write($"{context}: {exception.GetType().Name}: {exception.Message}{Environment.NewLine}{exception.StackTrace}");

    public static void Write(string message)
    {
        try
        {
            lock (Gate)
            {
                var info = new FileInfo(FilePath);
                if (info.Exists && info.Length > MaxBytes)
                {
                    File.Move(FilePath, FilePath + ".1", overwrite: true);
                }
                File.AppendAllText(FilePath, $"{DateTimeOffset.Now:O} {message}{Environment.NewLine}");
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            // Logging must never take the app down.
        }
    }
}
