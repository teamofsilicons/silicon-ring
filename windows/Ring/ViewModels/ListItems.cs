using Microsoft.UI.Xaml;
using Ring.Api;

namespace Ring.ViewModels;

internal static class Vis
{
    public static Visibility Of(bool visible) => visible ? Visibility.Visible : Visibility.Collapsed;
}

/// <summary>A row in Recents.</summary>
public sealed class RecentCallItem
{
    public RecentCallItem(Call call, string myUuid)
    {
        Call = call;
        var outgoing = call.CreatedBy.Uuid == myUuid;
        var others = new List<Account>();
        if (!outgoing)
        {
            others.Add(call.CreatedBy);
        }
        foreach (var account in call.Invitations.Select(i => i.To).Concat(call.Participants.Select(p => p.Account)))
        {
            if (account.Uuid != myUuid && others.All(o => o.Uuid != account.Uuid))
            {
                others.Add(account);
            }
        }
        Peer = others.FirstOrDefault() ?? call.CreatedBy;
        Title = others.Count > 1 ? $"{Peer.Label} +{others.Count - 1}" : Peer.Label;

        var mine = call.InvitationFor(myUuid);
        IsMissed = !outgoing && mine is not null && mine.State is InvitationStates.Missed or InvitationStates.Busy or InvitationStates.Cancelled && call.AnsweredAt is null;
        DirectionGlyph = outgoing ? "\uE7F2" : "\uE77E";

        var when = Format.When(call.CreatedAt);
        string outcome;
        if (call.AnsweredAt is { } answered && call.EndedAt is { } ended)
        {
            outcome = Format.Duration(ended - answered);
        }
        else if (call.State == CallStates.Active)
        {
            outcome = "ON CALL NOW";
        }
        else if (call.State is CallStates.Ringing)
        {
            outcome = "RINGING";
        }
        else if (call.VoicemailId is not null)
        {
            outcome = outgoing ? "LEFT A VOICEMAIL" : "VOICEMAIL";
        }
        else
        {
            outcome = call.EndReason switch
            {
                EndReasons.Missed => outgoing ? "NO ANSWER" : "MISSED",
                EndReasons.Declined => "DECLINED",
                EndReasons.Busy => "BUSY",
                EndReasons.Cancelled => outgoing ? "CANCELLED" : "MISSED",
                EndReasons.Failed => "FAILED",
                _ => outgoing ? "OUTGOING" : "INCOMING",
            };
        }
        Meta = $"{when} · {outcome}";
    }

    public Call Call { get; }

    public string Ringid => Call.Ringid;

    public Account Peer { get; }

    public string Title { get; }

    public string Meta { get; }

    public bool IsMissed { get; }

    public string DirectionGlyph { get; }

    public string CallTarget => Peer.Id;

    public Visibility MissedVisibility => Vis.Of(IsMissed);

    public Visibility NormalVisibility => Vis.Of(!IsMissed);

    public Visibility SiliconVisibility => Vis.Of(Peer.IsSilicon);
}

/// <summary>A row in Voicemail.</summary>
public sealed class VoicemailItem : ObservableObject
{
    private Voicemail _voicemail;
    private bool _isPlaying;

    public VoicemailItem(Voicemail voicemail)
    {
        _voicemail = voicemail;
    }

    public Voicemail Voicemail
    {
        get => _voicemail;
        set
        {
            if (Set(ref _voicemail, value))
            {
                OnPropertyChanged(nameof(UnreadVisibility));
                OnPropertyChanged(nameof(Preview));
                OnPropertyChanged(nameof(Meta));
            }
        }
    }

    public string Id => _voicemail.VoicemailId;

    public Account From => _voicemail.From;

    public string Title => _voicemail.From.Label;

    public Visibility SiliconVisibility => Vis.Of(_voicemail.From.IsSilicon);

    public Visibility UnreadVisibility => Vis.Of(!_voicemail.Read);

    public string ReasonText => ReasonLabel(_voicemail);

    public string Meta
    {
        get
        {
            var parts = new List<string> { ReasonLabel(_voicemail), Format.When(_voicemail.CreatedAt) };
            if (_voicemail.DurationMs is long ms)
            {
                parts.Add(Format.Clock(TimeSpan.FromMilliseconds(ms)));
            }
            return string.Join(" · ", parts);
        }
    }

    public string Preview => _voicemail.TranscriptStatus switch
    {
        _ when !string.IsNullOrWhiteSpace(_voicemail.Transcript) => _voicemail.Transcript!.Trim(),
        "pending" => "Transcribing…",
        "failed" => "No transcript",
        _ => "No transcript",
    };

    public bool IsPlaying
    {
        get => _isPlaying;
        set
        {
            if (Set(ref _isPlaying, value))
            {
                OnPropertyChanged(nameof(PlayGlyph));
            }
        }
    }

    public string PlayGlyph => _isPlaying ? "\uE769" : "\uE768";

    public bool CanPlay => _voicemail.AudioStatus == "ready";

    /// <summary>"MISSED", "BUSY", "DECLINED · In a meeting".</summary>
    public static string ReasonLabel(Voicemail voicemail) => voicemail.Reason switch
    {
        "declined" when !string.IsNullOrWhiteSpace(voicemail.DeclineReason) => $"DECLINED · {voicemail.DeclineReason}",
        "declined" => "DECLINED",
        "busy" => "BUSY",
        _ => "MISSED",
    };
}

/// <summary>A device in Settings.</summary>
public sealed class DeviceItem
{
    public DeviceItem(Device device)
    {
        Device = device;
    }

    public Device Device { get; }

    public string Id => Device.DeviceId;

    public string Name => string.IsNullOrWhiteSpace(Device.Name) ? "Unnamed device" : Device.Name!;

    public string Meta
    {
        get
        {
            var platform = Device.Platform switch
            {
                "ios" => "IPHONE",
                "ipados" => "IPAD",
                "macos" => "MAC",
                "android" => "ANDROID",
                "windows" => "WINDOWS",
                "linux" => "LINUX",
                "web" => "WEB",
                "cli" => "CLI",
                _ => "DEVICE",
            };
            var state = Device.Current ? "THIS DEVICE" : Device.Online ? "ONLINE" : Device.LastSeenAt is { } seen ? $"SEEN {Format.When(seen)}" : "OFFLINE";
            var ring = Device.RingEnabled ? "" : " · RINGING OFF";
            return $"{platform} · {state}{ring}";
        }
    }

    public string Glyph => Device.Platform switch
    {
        "ios" or "android" => "\uE8EA",
        "ipados" => "\uE70A",
        "macos" or "windows" or "linux" => "\uE977",
        _ => "\uE774",
    };

    public Visibility RevokeVisibility => Vis.Of(!Device.Current);
}

/// <summary>Someone in the dialer's list of recent people.</summary>
public sealed class PersonItem
{
    public PersonItem(Account account, string meta)
    {
        Account = account;
        Meta = meta;
    }

    public Account Account { get; }

    public string Name => Account.Label;

    public string Handle => Format.Pixel(Account.Id);

    public string Meta { get; }

    public Visibility SiliconVisibility => Vis.Of(Account.IsSilicon);
}

/// <summary>One event on the call details timeline.</summary>
public sealed class TimelineItem
{
    public TimelineItem(string time, string title, string? body, bool isSpeech)
    {
        Time = time;
        Title = title;
        Body = body ?? "";
        IsSpeech = isSpeech;
    }

    public string Time { get; }

    public string Title { get; }

    public string Body { get; }

    public bool IsSpeech { get; }

    public Visibility BodyVisibility => Vis.Of(Body.Length > 0);
}

/// <summary>An audio device choice; a null id is the system default.</summary>
public sealed class AudioDeviceChoice
{
    public AudioDeviceChoice(string? id, string name)
    {
        Id = id;
        Name = name;
    }

    public string? Id { get; }

    public string Name { get; }

    public override string ToString() => Name;
}

/// <summary>A voice option in Settings.</summary>
public sealed class VoiceChoice
{
    public VoiceChoice(Voice voice)
    {
        Voice = voice;
    }

    public Voice Voice { get; }

    public string Id => Voice.Id;

    public override string ToString() => string.IsNullOrWhiteSpace(Voice.Description) ? Voice.Name : $"{Voice.Name} · {Voice.Description}";
}
