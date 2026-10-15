using System.Collections.ObjectModel;
using Microsoft.UI.Dispatching;
using Ring.Api;
using Ring.Media;
using Ring.ViewModels;

namespace Ring.Services;

/// <summary>
/// The call state machine on this device: incoming invitations, the call this device carries
/// (with its LiveKit audio), calls carried by another of this account's devices, and the live
/// transcript. Everything runs on the UI thread.
/// </summary>
public sealed class CallCoordinator
{
    public static readonly IReadOnlyList<string> DeclineReasons =
        ["Can't talk right now", "In a meeting", "Call you back soon", "Text me instead"];

    private const int TranscriptKeep = 60;
    private const int MaxRejoinAttempts = 3;

    private readonly AppServices _app;
    private readonly Dictionary<string, Call> _open = [];
    // Invitations this device already answered, declined or silenced, so updates do not re-ring them.
    private readonly HashSet<string> _handled = [];
    private readonly DispatcherQueueTimer _tick;
    private ActiveCall? _active;
    private CallMedia? _media;
    private ElsewhereCall? _elsewhere;

    public CallCoordinator(AppServices app)
    {
        _app = app;
        _tick = app.Dispatcher.CreateTimer();
        _tick.Interval = TimeSpan.FromSeconds(1);
        _tick.IsRepeating = true;
        _tick.Tick += (_, _) => OnTick();
        _tick.Start();
    }

    public ActiveCall? Active => _active;

    public ObservableCollection<IncomingCall> Incoming { get; } = [];

    public ElsewhereCall? Elsewhere => _elsewhere;

    public event Action? ActiveChanged;

    public event Action<IncomingCall>? IncomingAdded;

    public event Action<IncomingCall>? IncomingRemoved;

    public event Action? ElsewhereChanged;

    /// <summary>A call ended or changed in a way the call history should show.</summary>
    public event Action? HistoryChanged;

    private string MyUuid => _app.Me?.Uuid ?? "";

    private string MyDeviceId => _app.DeviceId ?? "";

    // ---- server events ------------------------------------------------------------------------

    public void HandleEvent(RealtimeEvent frame)
    {
        switch (frame.Type)
        {
            case EventTypes.CallIncoming:
                Apply(frame.DataAs<CallIncomingData>().Call);
                break;
            case EventTypes.CallUpdated:
                Apply(frame.DataAs<CallEnvelope>().Call);
                break;
            case EventTypes.CallEnded:
                Apply(frame.DataAs<CallEnvelope>().Call);
                HistoryChanged?.Invoke();
                break;
            case EventTypes.CallRingingStopped:
                OnRingingStopped(frame.DataAs<RingingStoppedData>());
                break;
            case EventTypes.CallHandoff:
                OnHandoff(frame.DataAs<HandoffData>());
                break;
            case EventTypes.TranscriptEntry:
                var entry = frame.DataAs<TranscriptEntryData>();
                if (_active is { } active && active.Ringid == entry.Ringid)
                {
                    AddEntry(active, entry.Entry);
                }
                break;
            case EventTypes.TranscriptPartial:
                var partial = frame.DataAs<TranscriptPartialData>();
                if (_active is { } current && current.Ringid == partial.Ringid)
                {
                    AddPartial(current, partial);
                }
                break;
        }
    }

    /// <summary>After (re)connecting: catch up on calls that changed while the socket was down.</summary>
    public async Task ReconcileAsync()
    {
        if (!_app.IsSignedIn)
        {
            return;
        }
        List<Call> open;
        try
        {
            open = (await _app.Api.ListCallsAsync(new CallQuery(States: "ringing,active,voicemail", Direction: "all", Limit: 50))).Items;
        }
        catch (ApiException)
        {
            return;
        }

        var stillOpen = open.Select(c => c.Ringid).ToHashSet();
        var vanished = _open.Keys.Where(id => !stillOpen.Contains(id)).ToList();
        if (_active is { } active && !stillOpen.Contains(active.Ringid) && !vanished.Contains(active.Ringid))
        {
            vanished.Add(active.Ringid);
        }
        foreach (var ringid in vanished)
        {
            try
            {
                Apply(await _app.Api.GetCallAsync(ringid));
            }
            catch (ApiException)
            {
                _open.Remove(ringid);
            }
        }
        foreach (var call in open)
        {
            Apply(call);
        }

        // Our audio dropped while we were away (app restart, sleep): rejoin within the grace period.
        if (_active is null)
        {
            var mine = open.FirstOrDefault(c =>
                c.State is CallStates.Active or CallStates.Voicemail &&
                c.ParticipantFor(MyUuid) is { IsJoined: true } p && p.Device?.DeviceId == MyDeviceId);
            if (mine is not null)
            {
                await ClaimAsync(mine.Ringid, rejoin: true);
            }
        }
        HistoryChanged?.Invoke();
    }

    private void Apply(Call call)
    {
        if (call.IsOpen)
        {
            _open[call.Ringid] = call;
        }
        else
        {
            _open.Remove(call.Ringid);
        }

        UpdateIncoming(call);
        if (_active is { } active && active.Ringid == call.Ringid)
        {
            UpdateActive(active, call);
        }
        UpdateElsewhere();
    }

    // ---- incoming ---------------------------------------------------------------------------------

    private void UpdateIncoming(Call call)
    {
        var ringing = call.IsOpen ? call.RingingInvitationFor(MyUuid) : null;

        foreach (var existing in Incoming.Where(i => i.Ringid == call.Ringid).ToList())
        {
            if (ringing is null || ringing.InvitationId != existing.InvitationId || ringing.Silenced)
            {
                Dismiss(existing);
            }
            else
            {
                existing.UpdateInvitation(ringing);
                existing.Call = call;
            }
        }

        if (ringing is null || ringing.Silenced || !_app.RingEnabled || _handled.Contains(ringing.InvitationId) ||
            ringing.ExpiresAt <= _app.ServerNow || _active?.Ringid == call.Ringid ||
            Incoming.Any(i => i.InvitationId == ringing.InvitationId))
        {
            return;
        }

        var incoming = new IncomingCall(call, ringing, MyUuid);
        Incoming.Add(incoming);
        _app.Notifications.ShowIncoming(call, ringing, incoming.AlsoOnCall);
        _ = _app.Sounds.PlayLoopAsync(Sound.Ringtone);
        IncomingAdded?.Invoke(incoming);
    }

    private void Dismiss(IncomingCall incoming)
    {
        if (!Incoming.Remove(incoming))
        {
            return;
        }
        _app.Notifications.RemoveIncoming(incoming.InvitationId);
        if (Incoming.Count == 0 && _app.Sounds.IsLooping(Sound.Ringtone))
        {
            _app.Sounds.StopLoop();
        }
        IncomingRemoved?.Invoke(incoming);
    }

    private void OnRingingStopped(RingingStoppedData data)
    {
        _handled.Add(data.InvitationId);
        var incoming = Incoming.FirstOrDefault(i => i.InvitationId == data.InvitationId);
        Account? caller = incoming?.Caller;
        if (caller is null && _open.TryGetValue(data.Ringid, out var call))
        {
            caller = call.Invitations.FirstOrDefault(i => i.InvitationId == data.InvitationId)?.From;
        }
        if (incoming is not null)
        {
            Dismiss(incoming);
        }
        // A caller who hangs up before anyone answers is a missed call too, as on a phone.
        var missed = data.Reason == "missed" || (data.Reason == "cancelled" && incoming is not null);
        if (missed && caller is not null)
        {
            _app.Notifications.ShowMissed(caller, data.Ringid);
            HistoryChanged?.Invoke();
        }
    }

    public IncomingCall? FindIncoming(string? ringid, string? invitationId) =>
        Incoming.FirstOrDefault(i => (invitationId is not null && i.InvitationId == invitationId) || (invitationId is null && i.Ringid == ringid));

    public async Task AcceptAsync(IncomingCall incoming)
    {
        _handled.Add(incoming.InvitationId);
        Dismiss(incoming);
        if (_active is { IsEnded: false })
        {
            await HangUpAsync();
        }
        try
        {
            var result = await _app.Api.AcceptAsync(incoming.Ringid, incoming.InvitationId);
            _open[result.Call.Ringid] = result.Call;
            Begin(result.Call, result.Media, isOutgoing: false, primary: incoming.Caller);
        }
        catch (ApiException ex)
        {
            _app.ShowNotice(ex.Code switch
            {
                "invitation_not_ringing" => "That call was already answered or ended.",
                "invitation_expired" => "That call stopped ringing.",
                _ => ex.UserMessage,
            });
        }
    }

    /// <summary>Answers from a notification button, where only the ids are known.</summary>
    public async Task AcceptByIdAsync(string ringid, string? invitationId)
    {
        var incoming = FindIncoming(ringid, invitationId);
        if (incoming is not null)
        {
            await AcceptAsync(incoming);
            return;
        }
        try
        {
            var call = await _app.Api.GetCallAsync(ringid);
            var invitation = invitationId is null
                ? call.RingingInvitationFor(MyUuid)
                : call.Invitations.FirstOrDefault(i => i.InvitationId == invitationId && i.State == InvitationStates.Ringing);
            if (invitation is null)
            {
                _app.ShowNotice("That call is no longer ringing.");
                return;
            }
            await AcceptAsync(new IncomingCall(call, invitation, MyUuid));
        }
        catch (ApiException ex)
        {
            _app.ShowNotice(ex.UserMessage);
        }
    }

    /// <summary>Declines; <paramref name="reason"/> null declines without one.</summary>
    public async Task DeclineAsync(IncomingCall incoming, string? reason)
    {
        _handled.Add(incoming.InvitationId);
        Dismiss(incoming);
        try
        {
            await _app.Api.DeclineAsync(incoming.Ringid, incoming.InvitationId, reason);
            HistoryChanged?.Invoke();
        }
        catch (ApiException ex) when (ex.Code is "invitation_not_ringing" or "invitation_expired" or "call_ended")
        {
            // It stopped ringing on its own.
        }
        catch (ApiException ex)
        {
            _app.ShowNotice(ex.UserMessage);
        }
    }

    public async Task DeclineByIdAsync(string ringid, string? invitationId)
    {
        var incoming = FindIncoming(ringid, invitationId);
        if (incoming is not null)
        {
            await DeclineAsync(incoming, null);
            return;
        }
        try
        {
            await _app.Api.DeclineAsync(ringid, invitationId, null);
        }
        catch (ApiException)
        {
            // Already over.
        }
    }

    /// <summary>Stops the ringing on every device of this account without declining.</summary>
    public async Task SilenceAsync(IncomingCall incoming)
    {
        _handled.Add(incoming.InvitationId);
        Dismiss(incoming);
        try
        {
            await _app.Api.SilenceAsync(incoming.Ringid, incoming.InvitationId);
        }
        catch (ApiException)
        {
            // Already over; nothing to silence.
        }
    }

    // ---- placing and carrying calls ---------------------------------------------------------------

    public async Task<bool> StartCallAsync(string to)
    {
        if (_active is { IsEnded: false })
        {
            _app.ShowNotice("You're already on a call.");
            return false;
        }
        try
        {
            var created = await _app.Api.CreateCallAsync(to);
            _open[created.Call.Ringid] = created.Call;
            Begin(created.Call, created.Media, isOutgoing: true, primary: created.Invitation.To);
            return true;
        }
        catch (ApiException ex)
        {
            _app.ShowNotice(ex.Code switch
            {
                "account_not_found" => "Nobody on Silicon Accounts has that id.",
                "already_in_call" => "You're already on a call.",
                _ => ex.UserMessage,
            });
            return false;
        }
    }

    public async Task<bool> InviteAsync(string to)
    {
        if (_active is not { } active)
        {
            return false;
        }
        try
        {
            var invited = await _app.Api.InviteAsync(active.Ringid, to);
            Apply(invited.Call);
            return true;
        }
        catch (ApiException ex)
        {
            _app.ShowNotice(ex.Code == "account_not_found" ? "Nobody on Silicon Accounts has that id." : ex.UserMessage);
            return false;
        }
    }

    /// <summary>Moves a call carried by another of this account's devices to this one.</summary>
    public Task SwitchHereAsync(string ringid) => ClaimAsync(ringid, rejoin: false);

    /// <summary>Claims the call's audio for this device; false when the server refused or was unreachable.</summary>
    private async Task<bool> ClaimAsync(string ringid, bool rejoin)
    {
        if (_active is { IsEnded: false } current && current.Ringid != ringid)
        {
            _app.ShowNotice("Hang up your current call first.");
            return false;
        }
        var before = _active;
        try
        {
            var result = await _app.Api.ClaimMediaAsync(ringid);
            _open[ringid] = result.Call;
            if (_active is { IsEnded: false, LeftByUser: false } active && active.Ringid == ringid)
            {
                ConnectMedia(active, result.Media);
                return true;
            }
            if (rejoin && before?.Ringid == ringid)
            {
                // Hung up or ended while the claim was in flight; don't bring it back.
                return false;
            }
            var isOutgoing = result.Call.CreatedBy.Uuid == MyUuid;
            Begin(result.Call, result.Media, isOutgoing, PrimaryFor(result.Call, isOutgoing));
            _ = LoadTranscriptAsync(_active!);
            return true;
        }
        catch (ApiException ex)
        {
            if (!rejoin)
            {
                _app.ShowNotice(ex.UserMessage);
            }
            return false;
        }
    }

    public async Task HangUpAsync()
    {
        if (_active is not { IsEnded: false } active)
        {
            return;
        }
        active.LeftByUser = true;
        try
        {
            var call = await _app.Api.LeaveAsync(active.Ringid);
            _open[call.Ringid] = call;
        }
        catch (ApiException ex) when (ex.Code is "call_ended" or "not_participant" or "not_found" or "conflict")
        {
            // Already gone on the server.
        }
        catch (ApiException ex)
        {
            _app.ShowNotice(ex.UserMessage);
        }
        if (ReferenceEquals(_active, active))
        {
            Finish(active, active.Phase == CallPhase.Voicemail && active.IsOutgoing ? "MESSAGE SENT" : "CALL ENDED", null);
        }
        HistoryChanged?.Invoke();
    }

    public void ToggleMute()
    {
        if (_active is not { } active)
        {
            return;
        }
        active.IsMuted = !active.IsMuted;
        _media?.SetMuted(active.IsMuted);
    }

    public void SetInputDevice(string? id)
    {
        _app.Preferences.InputDeviceId = id;
        _app.Preferences.Save();
        _media?.SetInputDevice(id);
    }

    public void SetOutputDevice(string? id)
    {
        _app.Preferences.OutputDeviceId = id;
        _app.Preferences.Save();
        _media?.SetOutputDevice(id);
    }

    private Account PrimaryFor(Call call, bool isOutgoing)
    {
        if (!isOutgoing && call.CreatedBy.Uuid != MyUuid)
        {
            return call.CreatedBy;
        }
        var other = call.Invitations.FirstOrDefault(i => i.To.Uuid != MyUuid)?.To
                    ?? call.Participants.FirstOrDefault(p => p.Account.Uuid != MyUuid)?.Account;
        return other ?? call.CreatedBy;
    }

    private void Begin(Call call, MediaGrant? grant, bool isOutgoing, Account primary)
    {
        var active = new ActiveCall(call, isOutgoing, primary);
        _active = active;
        UpdateActive(active, call);
        ActiveChanged?.Invoke();
        UpdateElsewhere();
        if (grant is not null)
        {
            ConnectMedia(active, grant);
        }
        if (active.Phase == CallPhase.Active)
        {
            _ = LoadTranscriptAsync(active);
        }
    }

    private void UpdateActive(ActiveCall active, Call call)
    {
        var previous = active.Phase;
        active.Call = call;
        var mine = call.ParticipantFor(MyUuid);

        if (!call.IsOpen || mine is null || !mine.IsJoined)
        {
            if (!active.IsEnded)
            {
                Finish(active, EndStatus(active, call), EndDetail(active, call));
            }
            return;
        }

        if (_media is not null && mine.Device?.DeviceId is { Length: > 0 } carrier && carrier != MyDeviceId)
        {
            // Another of our devices took the call (the handoff event may arrive after this).
            MovedAway(active, mine.Device?.Name);
            return;
        }

        UpdatePeers(active, call);
        switch (call.State)
        {
            case CallStates.Ringing:
                active.Phase = active.MediaConnected || !active.IsOutgoing ? CallPhase.Ringing : CallPhase.Calling;
                active.Status = active.Phase == CallPhase.Ringing ? "RINGING" : "CALLING…";
                active.Detail = "";
                if (active.IsOutgoing)
                {
                    _ = _app.Sounds.PlayLoopAsync(Sound.Ringback);
                }
                break;
            case CallStates.Active:
                active.Phase = CallPhase.Active;
                active.Detail = "";
                if (previous != CallPhase.Active)
                {
                    StopRingback();
                    _ = _app.Sounds.PlayOnceAsync(Sound.Connected);
                    _ = LoadTranscriptAsync(active);
                }
                UpdateClock(active);
                break;
            case CallStates.Voicemail:
                active.Phase = CallPhase.Voicemail;
                StopRingback();
                active.Status = "LEAVE A MESSAGE AFTER THE BEEP";
                active.Detail = VoicemailDetail(active, call);
                break;
        }
    }

    private void UpdatePeers(ActiveCall active, Call call)
    {
        var wanted = new List<(Account Account, bool Ringing, string? Identity)>();
        foreach (var participant in call.JoinedParticipants.Where(p => p.Account.Uuid != MyUuid))
        {
            wanted.Add((participant.Account, false, participant.MediaIdentity));
        }
        foreach (var invitation in call.Invitations.Where(i => i.State == InvitationStates.Ringing && i.To.Uuid != MyUuid))
        {
            if (wanted.All(w => w.Account.Uuid != invitation.To.Uuid))
            {
                wanted.Add((invitation.To, true, null));
            }
        }

        for (var index = active.Peers.Count - 1; index >= 0; index--)
        {
            if (wanted.All(w => w.Account.Uuid != active.Peers[index].Uuid))
            {
                active.Peers.RemoveAt(index);
            }
        }
        foreach (var (account, ringing, identity) in wanted)
        {
            var peer = active.Peers.FirstOrDefault(p => p.Uuid == account.Uuid);
            if (peer is null)
            {
                peer = new CallPeer(account);
                active.Peers.Add(peer);
            }
            peer.Account = account;
            peer.IsRinging = ringing;
            peer.MediaIdentity = identity;
            if (account.Uuid == active.Primary.Uuid)
            {
                active.Primary.Account = account;
            }
        }
        active.RaiseTitleChanged();
    }

    private static string VoicemailDetail(ActiveCall active, Call call)
    {
        var invitation = call.Invitations.FirstOrDefault(i => i.To.Uuid == active.Primary.Uuid) ?? call.Invitations.FirstOrDefault();
        return invitation?.State switch
        {
            InvitationStates.Declined => string.IsNullOrWhiteSpace(invitation.Reason) ? "DECLINED" : $"DECLINED · {invitation.Reason}",
            InvitationStates.Busy => "BUSY",
            InvitationStates.Missed => "NO ANSWER",
            _ => "",
        };
    }

    private static string EndStatus(ActiveCall active, Call call)
    {
        if (active.Phase == CallPhase.Voicemail && active.IsOutgoing)
        {
            return "MESSAGE SENT";
        }
        return call.EndReason switch
        {
            EndReasons.Cancelled => "CANCELLED",
            EndReasons.Declined => "DECLINED",
            EndReasons.Missed => "NO ANSWER",
            EndReasons.Busy => "BUSY",
            EndReasons.Failed => "CALL FAILED",
            EndReasons.ServerRestart => "CALL DROPPED",
            _ => "CALL ENDED",
        };
    }

    private static string? EndDetail(ActiveCall active, Call call)
    {
        if (call.EndReason == EndReasons.Declined)
        {
            var reason = call.Invitations.FirstOrDefault(i => i.State == InvitationStates.Declined)?.Reason;
            return string.IsNullOrWhiteSpace(reason) ? null : reason;
        }
        return null;
    }

    private void Finish(ActiveCall active, string status, string? detail)
    {
        if (active.IsEnded)
        {
            return;
        }
        active.Phase = CallPhase.Ended;
        active.Status = status;
        active.Detail = detail ?? "";
        foreach (var peer in active.Peers)
        {
            peer.IsSpeaking = false;
        }
        StopRingback();
        _ = _app.Sounds.PlayOnceAsync(Sound.Ended);
        var media = _media;
        _media = null;
        if (media is not null)
        {
            _ = media.DisconnectAsync();
        }

        // Leave the outcome on screen for a moment, then return to the app.
        var close = _app.Dispatcher.CreateTimer();
        close.Interval = TimeSpan.FromSeconds(2.2);
        close.IsRepeating = false;
        close.Tick += (timer, _) =>
        {
            timer.Stop();
            if (ReferenceEquals(_active, active))
            {
                _active = null;
                ActiveChanged?.Invoke();
                UpdateElsewhere();
            }
        };
        close.Start();
        HistoryChanged?.Invoke();
    }

    /// <summary>Another of this account's devices took the call: drop our audio and say where it went.</summary>
    private void MovedAway(ActiveCall active, string? deviceName)
    {
        Finish(active, "MOVED", null);
        active.Banner = $"Moved to {(string.IsNullOrWhiteSpace(deviceName) ? "another device" : deviceName)}";
    }

    private void StopRingback()
    {
        if (_app.Sounds.IsLooping(Sound.Ringback))
        {
            _app.Sounds.StopLoop();
        }
    }

    // ---- media ----------------------------------------------------------------------------------

    private void ConnectMedia(ActiveCall active, MediaGrant? grant)
    {
        if (grant is null)
        {
            return;
        }
        var previous = _media;
        _media = null;
        if (previous is not null)
        {
            _ = previous.DisconnectAsync();
        }
        try
        {
            _media = CallMedia.Connect(grant, _app.Preferences.InputDeviceId, _app.Preferences.OutputDeviceId, active.IsMuted, (media, e) =>
            {
                // The microphone level (10 Hz) has no consumer on this screen.
                if (e.Type != MediaEventTypes.LocalLevel)
                {
                    _app.Post(() => OnMediaEvent(active, media, e));
                }
            });
        }
        catch (MediaUnavailableException ex)
        {
            _app.ShowNotice(ex.Message);
        }
    }

    private void OnMediaEvent(ActiveCall active, CallMedia media, MediaEvent e)
    {
        if (!ReferenceEquals(_media, media) || !ReferenceEquals(_active, active))
        {
            return;
        }
        switch (e.Type)
        {
            case MediaEventTypes.State:
                OnMediaState(active, e.State, e.Reason);
                break;
            case MediaEventTypes.ActiveSpeakers:
                var speaking = e.Identities.ToHashSet();
                foreach (var peer in active.Peers)
                {
                    peer.IsSpeaking = peer.MediaIdentity is not null && speaking.Contains(peer.MediaIdentity);
                }
                break;
        }
    }

    private void OnMediaState(ActiveCall active, string? state, string? reason)
    {
        switch (state)
        {
            case "connected":
                active.MediaConnected = true;
                active.RejoinAttempts = 0;
                if (active.Phase == CallPhase.Calling)
                {
                    active.Phase = CallPhase.Ringing;
                    active.Status = "RINGING";
                }
                else if (active.Phase == CallPhase.Active)
                {
                    UpdateClock(active);
                }
                break;
            case "reconnecting":
                if (active.Phase == CallPhase.Active)
                {
                    active.Status = "RECONNECTING…";
                }
                break;
            case "disconnected":
                active.MediaConnected = false;
                if (active.IsEnded || active.LeftByUser)
                {
                    return;
                }
                if (active.RejoinAttempts >= MaxRejoinAttempts)
                {
                    _app.ShowNotice(string.IsNullOrWhiteSpace(reason) ? "Call audio was lost." : $"Call audio was lost: {reason}");
                    _ = HangUpAsync();
                    return;
                }
                active.RejoinAttempts++;
                if (active.Phase == CallPhase.Active)
                {
                    active.Status = "RECONNECTING…";
                }
                _ = RejoinAfterDelayAsync(active);
                break;
        }
    }

    private async Task RejoinAfterDelayAsync(ActiveCall active)
    {
        while (true)
        {
            await Task.Delay(TimeSpan.FromSeconds(2 * active.RejoinAttempts));
            if (!IsCurrent(active) || await ClaimAsync(active.Ringid, rejoin: true))
            {
                return;
            }

            // The claim was refused or the server was unreachable. The call may simply be over
            // (its room closes when it ends, which also drops the audio), so ask the server.
            try
            {
                Apply(await _app.Api.GetCallAsync(active.Ringid));
            }
            catch (ApiException ex) when (ex.Status is 403 or 404 or 410)
            {
                if (IsCurrent(active))
                {
                    Finish(active, "CALL ENDED", null);
                }
            }
            catch (ApiException)
            {
                // Offline; try again below while attempts remain.
            }
            if (!IsCurrent(active))
            {
                return;
            }
            if (active.RejoinAttempts >= MaxRejoinAttempts)
            {
                _app.ShowNotice("Call audio was lost.");
                await HangUpAsync();
                return;
            }
            active.RejoinAttempts++;
        }
    }

    private bool IsCurrent(ActiveCall active) => ReferenceEquals(_active, active) && !active.IsEnded && !active.LeftByUser;

    // ---- handoff ----------------------------------------------------------------------------------

    private void OnHandoff(HandoffData data)
    {
        if (data.DeviceId == MyDeviceId)
        {
            return;
        }
        if (_active is { } active && active.Ringid == data.Ringid && !active.IsEnded)
        {
            string? deviceName = null;
            if (_open.TryGetValue(data.Ringid, out var call))
            {
                deviceName = call.ParticipantFor(MyUuid)?.Device is { DeviceId: var id, Name: var name } && id == data.DeviceId ? name : null;
            }
            MovedAway(active, deviceName);
        }
        UpdateElsewhere();
    }

    private void UpdateElsewhere()
    {
        ElsewhereCall? next = null;
        foreach (var call in _open.Values)
        {
            if (call.State != CallStates.Active || _active?.Ringid == call.Ringid)
            {
                continue;
            }
            var mine = call.ParticipantFor(MyUuid);
            if (mine is not { IsJoined: true } || mine.Device is null || mine.Device.DeviceId == MyDeviceId)
            {
                continue;
            }
            var others = call.JoinedParticipants.Where(p => p.Account.Uuid != MyUuid).Select(p => p.Account.Label).ToList();
            var with = others.Count == 0 ? "a call" : $"a call with {string.Join(", ", others)}";
            var where = string.IsNullOrWhiteSpace(mine.Device.Name) ? "another device" : mine.Device.Name;
            next = new ElsewhereCall(call.Ringid, $"On {with} on {where}");
            break;
        }
        if (next != _elsewhere)
        {
            _elsewhere = next;
            ElsewhereChanged?.Invoke();
        }
    }

    // ---- transcript -------------------------------------------------------------------------------

    private async Task LoadTranscriptAsync(ActiveCall active)
    {
        try
        {
            // Pages run oldest first; only the latest lines matter on the live screen.
            var latest = new List<TranscriptEntry>();
            long after = 0;
            for (var pages = 0; pages < 20; pages++)
            {
                var page = await _app.Api.GetTranscriptAsync(active.Ringid, after);
                latest.AddRange(page.Items);
                if (latest.Count > 30)
                {
                    latest.RemoveRange(0, latest.Count - 30);
                }
                if (page.Items.Count == 0 || page.NextAfterSeq <= after)
                {
                    break;
                }
                after = page.NextAfterSeq;
            }
            foreach (var entry in latest)
            {
                AddEntry(active, entry);
            }
        }
        catch (ApiException)
        {
            // Live entries still arrive over the socket.
        }
    }

    private void AddEntry(ActiveCall active, TranscriptEntry entry)
    {
        if (!active.SeenSeqs.Add(entry.Seq))
        {
            return;
        }
        var actor = entry.Actor;
        var who = actor is null ? "" : actor.Uuid == MyUuid ? "YOU" : Format.Pixel(actor.Label);
        TranscriptLine? line = entry.Kind switch
        {
            EntryKinds.Speech when !string.IsNullOrWhiteSpace(entry.Text) =>
                new TranscriptLine($"s:{entry.Seq}", who, entry.Text!.Trim(), false, false, actor?.Uuid),
            EntryKinds.Joined => SystemLine(entry, $"{who} JOINED"),
            EntryKinds.Left => SystemLine(entry, $"{who} LEFT"),
            EntryKinds.Invited => SystemLine(entry, $"{who} INVITED {Format.Pixel(entry.DataAccount("invitee")?.Label ?? "SOMEONE")}"),
            EntryKinds.Declined => SystemLine(entry, string.IsNullOrWhiteSpace(entry.DataString("reason"))
                ? $"{who} DECLINED"
                : $"{who} DECLINED · {entry.DataString("reason")}"),
            EntryKinds.Missed => SystemLine(entry, $"{who} DIDN'T ANSWER"),
            EntryKinds.Busy => SystemLine(entry, $"{who} WAS BUSY"),
            EntryKinds.Answered => SystemLine(entry, "ANSWERED"),
            EntryKinds.Note when !string.IsNullOrWhiteSpace(entry.Text) => SystemLine(entry, Format.Pixel(entry.Text!)),
            _ => null,
        };
        if (line is null)
        {
            return;
        }
        if (entry.Kind == EntryKinds.Speech && actor is not null)
        {
            // The final text replaces that speaker's interim line.
            for (var i = active.Transcript.Count - 1; i >= 0; i--)
            {
                if (active.Transcript[i].IsInterim && active.Transcript[i].SpeakerUuid == actor.Uuid)
                {
                    active.Transcript.RemoveAt(i);
                }
            }
        }
        active.Transcript.Add(line);
        TrimTranscript(active);
    }

    private static TranscriptLine SystemLine(TranscriptEntry entry, string text) =>
        new($"e:{entry.Seq}", "", text.Trim(), false, true, null);

    private void AddPartial(ActiveCall active, TranscriptPartialData partial)
    {
        if (string.IsNullOrWhiteSpace(partial.Text))
        {
            return;
        }
        var key = $"p:{partial.SegmentId}";
        var existing = active.Transcript.FirstOrDefault(l => l.Key == key);
        if (existing is not null)
        {
            existing.Text = partial.Text.Trim();
            return;
        }
        var who = partial.Speaker.Uuid == MyUuid ? "YOU" : Format.Pixel(partial.Speaker.Label);
        active.Transcript.Add(new TranscriptLine(key, who, partial.Text.Trim(), true, false, partial.Speaker.Uuid));
        TrimTranscript(active);
    }

    private static void TrimTranscript(ActiveCall active)
    {
        while (active.Transcript.Count > TranscriptKeep)
        {
            active.Transcript.RemoveAt(0);
        }
        var count = active.Transcript.Count;
        for (var i = 0; i < count; i++)
        {
            var age = count - 1 - i;
            active.Transcript[i].Opacity = Math.Max(0.18, 1.0 - age * 0.14);
        }
    }

    // ---- housekeeping -----------------------------------------------------------------------------

    private void OnTick()
    {
        if (_active is { Phase: CallPhase.Active } active)
        {
            UpdateClock(active);
        }
        var now = _app.ServerNow;
        foreach (var expired in Incoming.Where(i => i.Invitation.ExpiresAt.AddSeconds(3) < now).ToList())
        {
            Dismiss(expired);
        }
    }

    private void UpdateClock(ActiveCall active)
    {
        if (active.Call.AnsweredAt is { } answered)
        {
            active.Timer = Format.Clock(_app.ServerNow - answered);
        }
        if (active.Status != "RECONNECTING…" || active.MediaConnected)
        {
            active.Status = active.MediaConnected ? "ON CALL" : "CONNECTING…";
        }
    }

    /// <summary>Signs out or shuts down: drops audio and every ringing UI without touching the server.</summary>
    public async Task ResetAsync()
    {
        foreach (var incoming in Incoming.ToList())
        {
            Dismiss(incoming);
        }
        _app.Sounds.StopLoop();
        var media = _media;
        _media = null;
        if (media is not null)
        {
            await media.DisconnectAsync();
        }
        _open.Clear();
        _handled.Clear();
        if (_active is not null)
        {
            _active = null;
            ActiveChanged?.Invoke();
        }
        if (_elsewhere is not null)
        {
            _elsewhere = null;
            ElsewhereChanged?.Invoke();
        }
    }
}
