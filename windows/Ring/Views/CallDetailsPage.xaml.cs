using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Ring.Api;
using Ring.Services;
using Ring.ViewModels;
using Windows.Media.Core;

namespace Ring.Views;

/// <summary>Participants, outcome, decline reasons, the timeline with the transcript, and the recording.</summary>
public sealed partial class CallDetailsPage : Page
{
    private string? _callTarget;

    public CallDetailsPage()
    {
        InitializeComponent();
    }

    private static AppServices Services => App.Services;

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (e.Parameter is string ringid)
        {
            await LoadAsync(ringid);
        }
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        RecordingPlayer.MediaPlayer?.Pause();
        RecordingPlayer.Source = null;
    }

    private async Task LoadAsync(string ringid)
    {
        try
        {
            var call = await Services.Api.GetCallAsync(ringid);
            ShowCall(call);
            var transcript = await LoadTranscriptAsync(ringid);
            ShowTimeline(call, transcript);
            await LoadRecordingAsync(call);
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
        }
    }

    private async Task<List<TranscriptEntry>> LoadTranscriptAsync(string ringid)
    {
        var entries = new List<TranscriptEntry>();
        long after = 0;
        for (var page = 0; page < 20; page++)
        {
            var result = await Services.Api.GetTranscriptAsync(ringid, after);
            entries.AddRange(result.Items);
            if (result.Items.Count == 0 || result.NextAfterSeq <= after)
            {
                break;
            }
            after = result.NextAfterSeq;
        }
        return entries;
    }

    private void ShowCall(Call call)
    {
        var me = Services.Me?.Uuid ?? "";
        var people = new List<PersonItem>();
        foreach (var account in new[] { call.CreatedBy }
                     .Concat(call.Participants.Select(p => p.Account))
                     .Concat(call.Invitations.Select(i => i.To)))
        {
            if (account.Uuid == me || people.Any(p => p.Account.Uuid == account.Uuid))
            {
                continue;
            }
            var role = account.Uuid == call.CreatedBy.Uuid ? "CALLED" : InvitationRole(call, account);
            people.Add(new PersonItem(account, $"{Format.Pixel(account.Id)} · {role}"));
        }
        PeopleList.ItemsSource = people;
        _callTarget = people.FirstOrDefault()?.Account.Id;
        CallAgainButton.Visibility = _callTarget is null ? Visibility.Collapsed : Visibility.Visible;

        var item = new RecentCallItem(call, me);
        OutcomeText.Text = item.Meta;
        var waited = call.AnsweredAt is { } answered ? $" · ANSWERED AFTER {Format.Duration(answered - call.CreatedAt)}" : "";
        WhenText.Text = $"{call.CreatedAt.ToLocalTime():f}".ToUpperInvariant() + waited;

        var reasons = call.Invitations
            .Where(i => i.State == InvitationStates.Declined)
            .Select(i => string.IsNullOrWhiteSpace(i.Reason) ? $"{i.To.Label} declined." : $"{i.To.Label} declined: {i.Reason}")
            .ToList();
        ReasonsText.Text = string.Join("\n", reasons);
        ReasonsText.Visibility = reasons.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    private static string InvitationRole(Call call, Account account)
    {
        var invitation = call.Invitations.LastOrDefault(i => i.To.Uuid == account.Uuid);
        return invitation?.State switch
        {
            InvitationStates.Accepted => "ANSWERED",
            InvitationStates.Declined => "DECLINED",
            InvitationStates.Missed => "MISSED",
            InvitationStates.Busy => "BUSY",
            InvitationStates.Cancelled => "CANCELLED",
            InvitationStates.Ringing => "RINGING",
            _ => "JOINED",
        };
    }

    private void ShowTimeline(Call call, List<TranscriptEntry> entries)
    {
        var me = Services.Me?.Uuid ?? "";
        var items = new List<TimelineItem>();
        foreach (var entry in entries)
        {
            var time = entry.OffsetMs is long offset && offset >= 0
                ? Format.Clock(TimeSpan.FromMilliseconds(offset))
                : entry.At.ToLocalTime().ToString("HH:mm");
            var who = entry.Actor is null ? "" : entry.Actor.Uuid == me ? "YOU" : Format.Pixel(entry.Actor.Label);
            TimelineItem? item = entry.Kind switch
            {
                EntryKinds.CallStarted => new TimelineItem(time, $"{who} STARTED THE CALL".Trim(), null, false),
                EntryKinds.Answered => new TimelineItem(time, "ANSWERED", null, false),
                EntryKinds.Joined => new TimelineItem(time, $"{who} JOINED", null, false),
                EntryKinds.Left => new TimelineItem(time, $"{who} LEFT", null, false),
                EntryKinds.Invited => new TimelineItem(time, $"{who} INVITED {Format.Pixel(entry.DataAccount("invitee")?.Label ?? "SOMEONE")}", null, false),
                EntryKinds.Declined => new TimelineItem(time, $"{who} DECLINED", entry.DataString("reason"), false),
                EntryKinds.Missed => new TimelineItem(time, $"{who} DIDN'T ANSWER", null, false),
                EntryKinds.Busy => new TimelineItem(time, $"{who} WAS BUSY", null, false),
                EntryKinds.Speech => new TimelineItem(time, who, entry.Text, true),
                EntryKinds.Voicemail => new TimelineItem(time, $"{who} LEFT A VOICEMAIL".Trim(), null, false),
                EntryKinds.CallEnded => new TimelineItem(time, "CALL ENDED", null, false),
                EntryKinds.Note => new TimelineItem(time, "NOTE", entry.Text, false),
                _ => null,
            };
            if (item is not null)
            {
                items.Add(item);
            }
        }
        TimelineList.ItemsSource = items;
        TimelineEmpty.Visibility = items.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    private async Task LoadRecordingAsync(Call call)
    {
        if (call.Recording.Status == "none")
        {
            RecordingCard.Visibility = Visibility.Collapsed;
            return;
        }
        RecordingCard.Visibility = Visibility.Visible;
        try
        {
            var recording = await Services.Api.GetRecordingAsync(call.Ringid);
            if (recording.Status == "ready" && !string.IsNullOrEmpty(recording.Url))
            {
                RecordingPlayer.Source = MediaSource.CreateFromUri(new Uri(recording.Url));
                RecordingPlayer.Visibility = Visibility.Visible;
                RecordingStatus.Text = recording.DurationMs is long ms ? Format.Clock(TimeSpan.FromMilliseconds(ms)) : "";
            }
            else
            {
                RecordingPlayer.Visibility = Visibility.Collapsed;
                RecordingStatus.Text = recording.Status switch
                {
                    "recording" => "Recording while the call is on.",
                    "processing" => "The recording is being processed.",
                    "failed" => "The recording failed.",
                    _ => "No recording.",
                };
            }
        }
        catch (ApiException)
        {
            RecordingStatus.Text = "The recording is not available.";
        }
    }

    private async void OnCallAgainClick(object sender, RoutedEventArgs e)
    {
        if (_callTarget is not null)
        {
            await Services.Calls.StartCallAsync(_callTarget);
        }
    }
}
