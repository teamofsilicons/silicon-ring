using System.Security;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;
using Ring.Api;

namespace Ring.Services;

/// <summary>What the user chose on one of Ring's notifications.</summary>
public sealed record NotificationAction(string Action, string? Ringid, string? InvitationId, string? VoicemailId, string? To);

/// <summary>
/// Windows notifications through the Windows App SDK: the incoming-call toast (IncomingCall
/// scenario with Answer and Decline), missed calls and new voicemail.
/// </summary>
public sealed class CallNotifications
{
    private const string CallGroup = "calls";
    private bool _registered;

    /// <summary>Raised on a background thread when a notification or one of its buttons is used.</summary>
    public event Action<NotificationAction>? Invoked;

    public void Register()
    {
        if (_registered)
        {
            return;
        }
        try
        {
            if (!AppNotificationManager.IsSupported())
            {
                return;
            }
            var manager = AppNotificationManager.Default;
            manager.NotificationInvoked += OnInvoked;
            try
            {
                manager.Register("Ring", new Uri(AppPaths.Asset("RingIcon.png")));
            }
            catch (Exception)
            {
                manager.Register();
            }
            _registered = true;
        }
        catch (Exception)
        {
            // Notifications are an extra; the ringing window still works without them.
        }
    }

    public void Unregister()
    {
        if (!_registered)
        {
            return;
        }
        try
        {
            AppNotificationManager.Default.Unregister();
        }
        catch (Exception)
        {
            // Shutting down anyway.
        }
        _registered = false;
    }

    /// <summary>Parses the argument string of a toast or button, e.g. <c>action=answer;ringid=…</c>.</summary>
    public static NotificationAction? ParseArguments(string? argument)
    {
        if (string.IsNullOrWhiteSpace(argument))
        {
            return null;
        }
        var values = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var part in argument.Split(';', StringSplitOptions.RemoveEmptyEntries))
        {
            var eq = part.IndexOf('=');
            if (eq > 0)
            {
                values[part[..eq]] = Uri.UnescapeDataString(part[(eq + 1)..]);
            }
        }
        return values.TryGetValue("action", out var action)
            ? new NotificationAction(action, values.GetValueOrDefault("ringid"), values.GetValueOrDefault("invitation"),
                values.GetValueOrDefault("voicemail"), values.GetValueOrDefault("to"))
            : null;
    }

    /// <summary>
    /// The incoming-call toast. Its own audio is silent because the ringing window plays the Ring
    /// ringtone; the toast stays on screen until it is answered, declined or removed.
    /// </summary>
    public void ShowIncoming(Call call, Invitation invitation, string? alsoOnCall)
    {
        if (!_registered)
        {
            return;
        }
        var caller = invitation.From;
        var subtitle = alsoOnCall is null ? $"{caller.Id} is calling" : $"{caller.Id} is calling · with {alsoOnCall}";
        var common = $"ringid={Escape(call.Ringid)};invitation={Escape(invitation.InvitationId)}";
        var xml = $"""
            <toast scenario="incomingCall" useButtonStyle="true" launch="{Attr("action=open;" + common)}">
              <visual>
                <binding template="ToastGeneric">
                  <text hint-callScenarioCenterAlign="true">{Text(caller.Label)}</text>
                  <text hint-callScenarioCenterAlign="true">{Text(subtitle)}</text>
                </binding>
              </visual>
              <actions>
                <action content="Answer" arguments="{Attr("action=answer;" + common)}" hint-buttonStyle="Success" activationType="foreground" />
                <action content="Decline" arguments="{Attr("action=decline;" + common)}" hint-buttonStyle="Critical" activationType="foreground" />
              </actions>
              <audio silent="true" />
            </toast>
            """;
        Show(new AppNotification(xml) { Tag = Tag(invitation.InvitationId), Group = CallGroup, ExpiresOnReboot = true });
    }

    public void RemoveIncoming(string invitationId)
    {
        if (!_registered)
        {
            return;
        }
        _ = RemoveAsync(Tag(invitationId));
    }

    public void ShowMissed(Account caller, string ringid)
    {
        if (!_registered)
        {
            return;
        }
        var builder = new AppNotificationBuilder()
            .AddArgument("action", "open")
            .AddArgument("ringid", ringid)
            .AddText($"Missed call from {caller.Label}")
            .AddText(caller.Id)
            .AddButton(new AppNotificationButton("Call back")
                .AddArgument("action", "callback")
                .AddArgument("ringid", ringid)
                .AddArgument("to", caller.Id));
        Show(builder.BuildNotification());
    }

    public void ShowVoicemail(Voicemail voicemail)
    {
        if (!_registered)
        {
            return;
        }
        var preview = string.IsNullOrWhiteSpace(voicemail.Transcript) ? "New voicemail" : voicemail.Transcript!.Trim();
        var builder = new AppNotificationBuilder()
            .AddArgument("action", "voicemail")
            .AddArgument("voicemail", voicemail.VoicemailId)
            .AddText($"Voicemail from {voicemail.From.Label}")
            .AddText(preview.Length > 180 ? preview[..180] + "…" : preview);
        Show(builder.BuildNotification());
    }

    private static void Show(AppNotification notification)
    {
        try
        {
            AppNotificationManager.Default.Show(notification);
        }
        catch (Exception)
        {
            // Notifications turned off for Ring, or Focus Assist; nothing to do.
        }
    }

    private static async Task RemoveAsync(string tag)
    {
        try
        {
            await AppNotificationManager.Default.RemoveByTagAndGroupAsync(tag, CallGroup);
        }
        catch (Exception)
        {
            // Already gone.
        }
    }

    private void OnInvoked(AppNotificationManager sender, AppNotificationActivatedEventArgs args)
    {
        NotificationAction? action = null;
        try
        {
            action = ParseArguments(args.Argument);
            if (action is null && args.Arguments is { Count: > 0 } map)
            {
                action = new NotificationAction(
                    map.TryGetValue("action", out var a) ? a : "open",
                    map.TryGetValue("ringid", out var r) ? r : null,
                    map.TryGetValue("invitation", out var i) ? i : null,
                    map.TryGetValue("voicemail", out var v) ? v : null,
                    map.TryGetValue("to", out var t) ? t : null);
            }
        }
        catch (Exception)
        {
            // Unreadable arguments: just bring the app forward.
        }
        Invoked?.Invoke(action ?? new NotificationAction("open", null, null, null, null));
    }

    // Tags are limited to 64 characters; invitation ids are far shorter.
    private static string Tag(string invitationId) => invitationId.Length <= 64 ? invitationId : invitationId[..64];

    private static string Escape(string value) => Uri.EscapeDataString(value);

    private static string Attr(string value) => SecurityElement.Escape(value) ?? "";

    private static string Text(string value) => SecurityElement.Escape(value) ?? "";
}
