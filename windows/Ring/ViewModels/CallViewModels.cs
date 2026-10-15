using System.Collections.ObjectModel;
using Microsoft.UI.Xaml;
using Ring.Api;

namespace Ring.ViewModels;

public enum CallPhase
{
    /// <summary>Outgoing: placing the call and joining the room.</summary>
    Calling,
    /// <summary>Outgoing: the callee's devices are ringing.</summary>
    Ringing,
    /// <summary>Answered; audio connected or connecting.</summary>
    Active,
    /// <summary>The caller is leaving a voicemail after the beep.</summary>
    Voicemail,
    Ended,
}

/// <summary>Someone shown on the call screen: a joined participant or a person still being rung.</summary>
public sealed class CallPeer : ObservableObject
{
    private Account _account;
    private bool _isRinging;
    private bool _isSpeaking;

    public CallPeer(Account account)
    {
        _account = account;
    }

    public string Uuid => _account.Uuid;

    public Account Account
    {
        get => _account;
        set
        {
            if (Set(ref _account, value))
            {
                OnPropertyChanged(nameof(Name));
                OnPropertyChanged(nameof(Handle));
                OnPropertyChanged(nameof(SiliconBadgeVisibility));
            }
        }
    }

    public string Name => _account.Label;

    /// <summary>The id under the name; empty when the id is already the name (no display name).</summary>
    public string Handle => _account.HasDisplayName ? _account.Id : "";

    public Visibility SiliconBadgeVisibility => _account.IsSilicon ? Visibility.Visible : Visibility.Collapsed;

    public string? MediaIdentity { get; set; }

    public bool IsRinging
    {
        get => _isRinging;
        set
        {
            if (Set(ref _isRinging, value))
            {
                OnPropertyChanged(nameof(StatusText));
                OnPropertyChanged(nameof(PeerOpacity));
            }
        }
    }

    public bool IsSpeaking
    {
        get => _isSpeaking;
        set => Set(ref _isSpeaking, value);
    }

    public string StatusText => _isRinging ? "RINGING" : Format.Pixel(_account.Id);

    public double PeerOpacity => _isRinging ? 0.55 : 1.0;
}

/// <summary>One line of the live transcript.</summary>
public sealed class TranscriptLine : ObservableObject
{
    private string _text;
    private bool _isInterim;
    private double _opacity = 1;

    public TranscriptLine(string key, string speaker, string text, bool isInterim, bool isSystem, string? speakerUuid)
    {
        Key = key;
        Speaker = speaker;
        _text = text;
        _isInterim = isInterim;
        IsSystem = isSystem;
        SpeakerUuid = speakerUuid;
    }

    public string Key { get; }

    public string Speaker { get; }

    public string? SpeakerUuid { get; }

    public bool IsSystem { get; }

    public string Text
    {
        get => _text;
        set => Set(ref _text, value);
    }

    public bool IsInterim
    {
        get => _isInterim;
        set
        {
            if (Set(ref _isInterim, value))
            {
                OnPropertyChanged(nameof(TextOpacity));
            }
        }
    }

    /// <summary>Interim speech is muted until the final text arrives.</summary>
    public double TextOpacity => _isInterim ? 0.55 : 1.0;

    public Visibility SpeakerVisibility => IsSystem ? Visibility.Collapsed : Visibility.Visible;

    public Visibility TextVisibility => IsSystem ? Visibility.Collapsed : Visibility.Visible;

    public Visibility SystemVisibility => IsSystem ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Older lines fade as they move up.</summary>
    public double Opacity
    {
        get => _opacity;
        set => Set(ref _opacity, value);
    }
}

/// <summary>The call this device carries: outgoing, in progress, or leaving a voicemail.</summary>
public sealed class ActiveCall : ObservableObject
{
    private Call _call;
    private CallPhase _phase;
    private string _status = "CALLING…";
    private string _detail = "";
    private string _timer = "";
    private bool _isMuted;
    private string? _banner;
    private bool _mediaConnected;

    public ActiveCall(Call call, bool isOutgoing, Account primary)
    {
        _call = call;
        IsOutgoing = isOutgoing;
        Primary = new CallPeer(primary);
        _phase = isOutgoing ? CallPhase.Calling : CallPhase.Active;
    }

    public string Ringid => _call.Ringid;

    public Call Call
    {
        get => _call;
        set => Set(ref _call, value);
    }

    public bool IsOutgoing { get; }

    /// <summary>The person the hero screen is about: the callee, or whoever called us.</summary>
    public CallPeer Primary { get; }

    public ObservableCollection<CallPeer> Peers { get; } = [];

    public ObservableCollection<TranscriptLine> Transcript { get; } = [];

    public HashSet<long> SeenSeqs { get; } = [];

    public int RejoinAttempts { get; set; }

    public bool LeftByUser { get; set; }

    public CallPhase Phase
    {
        get => _phase;
        set
        {
            if (Set(ref _phase, value))
            {
                OnPropertyChanged(nameof(HeroVisibility));
                OnPropertyChanged(nameof(ConversationVisibility));
                OnPropertyChanged(nameof(RingsActive));
                OnPropertyChanged(nameof(DoneVisibility));
                OnPropertyChanged(nameof(ControlsVisibility));
                OnPropertyChanged(nameof(IsEnded));
            }
        }
    }

    public bool IsEnded => _phase == CallPhase.Ended;

    /// <summary>Pixel status line: CALLING…, RINGING, ON CALL, LEAVE A MESSAGE AFTER THE BEEP, CALL ENDED.</summary>
    public string Status
    {
        get => _status;
        set => Set(ref _status, value);
    }

    /// <summary>A second pixel line, e.g. "DECLINED · In a meeting".</summary>
    public string Detail
    {
        get => _detail;
        set
        {
            if (Set(ref _detail, value))
            {
                OnPropertyChanged(nameof(DetailVisibility));
            }
        }
    }

    public Visibility DetailVisibility => string.IsNullOrEmpty(_detail) ? Visibility.Collapsed : Visibility.Visible;

    public string Timer
    {
        get => _timer;
        set => Set(ref _timer, value);
    }

    public bool IsMuted
    {
        get => _isMuted;
        set
        {
            if (Set(ref _isMuted, value))
            {
                OnPropertyChanged(nameof(MuteLabel));
                OnPropertyChanged(nameof(MutedVisibility));
                OnPropertyChanged(nameof(UnmutedVisibility));
            }
        }
    }

    public string MuteLabel => _isMuted ? "UNMUTE" : "MUTE";

    public Visibility MutedVisibility => _isMuted ? Visibility.Visible : Visibility.Collapsed;

    public Visibility UnmutedVisibility => _isMuted ? Visibility.Collapsed : Visibility.Visible;

    public bool MediaConnected
    {
        get => _mediaConnected;
        set => Set(ref _mediaConnected, value);
    }

    /// <summary>A beige banner over the controls, e.g. "Moved to iPhone".</summary>
    public string? Banner
    {
        get => _banner;
        set
        {
            if (Set(ref _banner, value))
            {
                OnPropertyChanged(nameof(BannerVisibility));
            }
        }
    }

    public Visibility BannerVisibility => string.IsNullOrEmpty(_banner) ? Visibility.Collapsed : Visibility.Visible;

    /// <summary>The big centred avatar while calling, ringing, in voicemail and at the end.</summary>
    public Visibility HeroVisibility => _phase == CallPhase.Active ? Visibility.Collapsed : Visibility.Visible;

    /// <summary>Participants strip and transcript once the call is answered.</summary>
    public Visibility ConversationVisibility => _phase == CallPhase.Active ? Visibility.Visible : Visibility.Collapsed;

    public Visibility DoneVisibility => _phase == CallPhase.Voicemail ? Visibility.Visible : Visibility.Collapsed;

    public Visibility ControlsVisibility => _phase == CallPhase.Ended ? Visibility.Collapsed : Visibility.Visible;

    public bool RingsActive => _phase is CallPhase.Calling or CallPhase.Ringing or CallPhase.Voicemail;

    public string Title => Peers.Count switch
    {
        0 => Primary.Name,
        1 => Peers[0].Name,
        2 => $"{Peers[0].Name} and {Peers[1].Name}",
        _ => $"{Peers[0].Name} and {Peers.Count - 1} others",
    };

    public void RaiseTitleChanged() => OnPropertyChanged(nameof(Title));
}

/// <summary>A call ringing for this account (shown in the ringing window and as a toast).</summary>
public sealed class IncomingCall : ObservableObject
{
    private Call _call;

    public IncomingCall(Call call, Invitation invitation, string myUuid)
    {
        _call = call;
        Invitation = invitation;
        MyUuid = myUuid;
    }

    public string Ringid => _call.Ringid;

    public string InvitationId => Invitation.InvitationId;

    public Invitation Invitation { get; private set; }

    public string MyUuid { get; }

    public Account Caller => Invitation.From;

    public Call Call
    {
        get => _call;
        set
        {
            if (Set(ref _call, value))
            {
                OnPropertyChanged(nameof(AlsoOnCall));
                OnPropertyChanged(nameof(AlsoOnCallVisibility));
            }
        }
    }

    public void UpdateInvitation(Invitation invitation) => Invitation = invitation;

    /// <summary>"WITH ALICE, BOB" when others besides the caller are already on the call.</summary>
    public string? AlsoOnCall
    {
        get
        {
            var others = _call.JoinedParticipants
                .Where(p => p.Account.Uuid != Caller.Uuid && p.Account.Uuid != MyUuid)
                .Select(p => p.Account.Label)
                .ToList();
            return others.Count == 0 ? null : string.Join(", ", others);
        }
    }

    public Visibility AlsoOnCallVisibility => AlsoOnCall is null ? Visibility.Collapsed : Visibility.Visible;
}

/// <summary>An active call this account is on from another device ("Switch to this PC").</summary>
public sealed record ElsewhereCall(string Ringid, string Message);
