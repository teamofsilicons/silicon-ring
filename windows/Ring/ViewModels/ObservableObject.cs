using System.ComponentModel;
using System.Globalization;
using System.Runtime.CompilerServices;
using System.Text;
using Ring.Api;

namespace Ring.ViewModels;

public abstract class ObservableObject : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;

    protected bool Set<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value))
        {
            return false;
        }
        field = value;
        OnPropertyChanged(name);
        return true;
    }

    protected void OnPropertyChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}

/// <summary>Text formatting shared by every screen (pixel-font meta lines are uppercase).</summary>
public static class Format
{
    /// <summary>"4:05" or "1:02:03".</summary>
    public static string Clock(TimeSpan span)
    {
        if (span < TimeSpan.Zero)
        {
            span = TimeSpan.Zero;
        }
        return span.TotalHours >= 1
            ? $"{(int)span.TotalHours}:{span.Minutes:00}:{span.Seconds:00}"
            : $"{span.Minutes:00}:{span.Seconds:00}";
    }

    /// <summary>"4 MIN", "35 SEC", "1 H 4 MIN".</summary>
    public static string Duration(TimeSpan span)
    {
        if (span.TotalSeconds < 60)
        {
            return $"{Math.Max(0, (int)span.TotalSeconds)} SEC";
        }
        if (span.TotalHours < 1)
        {
            return $"{(int)span.TotalMinutes} MIN";
        }
        return span.Minutes == 0 ? $"{(int)span.TotalHours} H" : $"{(int)span.TotalHours} H {span.Minutes} MIN";
    }

    /// <summary>"09:30" today, "MON 09:30" this week, "OCT 3" this year, else "OCT 3 2025".</summary>
    public static string When(DateTimeOffset at)
    {
        var local = at.ToLocalTime();
        var now = DateTimeOffset.Now;
        var culture = CultureInfo.CurrentCulture;
        var time = local.ToString("t", culture);
        if (local.Date == now.Date)
        {
            return time.ToUpperInvariant();
        }
        if (local.Date == now.Date.AddDays(-1))
        {
            return $"YESTERDAY {time}".ToUpperInvariant();
        }
        if ((now.Date - local.Date).TotalDays < 7)
        {
            return $"{local.ToString("ddd", culture)} {time}".ToUpperInvariant();
        }
        var format = local.Year == now.Year ? "MMM d" : "MMM d yyyy";
        return local.ToString(format, culture).ToUpperInvariant();
    }

    /// <summary>The avatar initial: the first letter of the name, or of the handle after "c:"/"si:".</summary>
    public static string Initial(Account? account)
    {
        if (account is null)
        {
            return "?";
        }
        var source = account.HasDisplayName ? account.DisplayName!.Trim() : StripPrefix(account.Id);
        foreach (var rune in source.EnumerateRunes())
        {
            if (Rune.IsLetterOrDigit(rune))
            {
                return rune.ToString().ToUpper(CultureInfo.CurrentCulture);
            }
        }
        return "?";
    }

    public static string StripPrefix(string id)
    {
        var colon = id.IndexOf(':');
        return colon >= 0 && colon < id.Length - 1 ? id[(colon + 1)..] : id;
    }

    public static string Pixel(string text) => text.ToUpperInvariant();

    /// <summary>How a device is named in "Moved to …" and "On a call … on …": its name, else a word for its platform.</summary>
    public static string DeviceLabel(DeviceRef? device)
    {
        if (device is null)
        {
            return "another device";
        }
        if (!string.IsNullOrWhiteSpace(device.Name))
        {
            return device.Name.Trim();
        }
        return device.Platform switch
        {
            "ios" => "iPhone",
            "ipados" => "iPad",
            "macos" => "Mac",
            "android" => "Android phone",
            "windows" => "Windows PC",
            "linux" => "Linux computer",
            "web" => "the web",
            "cli" => "the command line",
            _ => "another device",
        };
    }

    /// <summary>Normalizes what the user typed into an actor reference: trims, drops "@", lowercases the prefix.</summary>
    public static string? ActorReference(string input)
    {
        var s = input.Trim().TrimStart('@').Trim();
        if (s.Length == 0)
        {
            return null;
        }
        var lower = s.ToLowerInvariant();
        if (lower.StartsWith("c:", StringComparison.Ordinal) || lower.StartsWith("si:", StringComparison.Ordinal))
        {
            var handle = lower[(lower.IndexOf(':') + 1)..];
            return handle.Length > 0 && handle.All(c => char.IsAsciiLetterOrDigit(c) || c is '_' or '-' or '.') ? lower : null;
        }
        return Guid.TryParse(lower, out _) && lower.Length == 36 ? lower : null;
    }
}
