using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Ring.Api;
using Ring.Services;
using Ring.ViewModels;
using Windows.Media.Core;

namespace Ring.Views;

/// <summary>From, the reason, a player with a scrubber, the transcript and Call back.</summary>
public sealed partial class VoicemailDetailPage : Page
{
    private Voicemail? _voicemail;

    public VoicemailDetailPage()
    {
        InitializeComponent();
    }

    private static AppServices Services => App.Services;

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (e.Parameter is string id)
        {
            await LoadAsync(id);
        }
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        Player.MediaPlayer?.Pause();
        Player.Source = null;
    }

    private async Task LoadAsync(string id)
    {
        try
        {
            var voicemail = await Services.Api.GetVoicemailAsync(id);
            Show(voicemail);
            if (!voicemail.Read)
            {
                _voicemail = await Services.Api.SetVoicemailReadAsync(id, true);
                Services.NotifyVoicemailsChanged();
            }
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
        }
    }

    private void Show(Voicemail voicemail)
    {
        _voicemail = voicemail;
        FromAvatar.Account = voicemail.From;
        FromName.Text = voicemail.From.Label;
        FromHandle.Text = Format.Pixel(voicemail.From.Id);
        SiliconBadge.Visibility = voicemail.From.IsSilicon ? Visibility.Visible : Visibility.Collapsed;
        var reason = VoicemailItem.ReasonLabel(voicemail);
        ReasonText.Text = voicemail.DurationMs is long ms
            ? $"{reason} · {Format.When(voicemail.CreatedAt)} · {Format.Clock(TimeSpan.FromMilliseconds(ms))}"
            : $"{reason} · {Format.When(voicemail.CreatedAt)}";

        TranscriptText.Text = !string.IsNullOrWhiteSpace(voicemail.Transcript)
            ? voicemail.Transcript!.Trim()
            : voicemail.TranscriptStatus == "pending" ? "Transcribing…" : "No transcript.";

        if (!string.IsNullOrEmpty(voicemail.AudioUrl))
        {
            Player.Source = MediaSource.CreateFromUri(new Uri(voicemail.AudioUrl));
            Player.Visibility = Visibility.Visible;
            AudioStatus.Visibility = Visibility.Collapsed;
        }
        else
        {
            Player.Visibility = Visibility.Collapsed;
            AudioStatus.Text = voicemail.AudioStatus == "failed" ? "The audio could not be processed." : "The audio is still processing.";
            AudioStatus.Visibility = Visibility.Visible;
        }
    }

    private async void OnCallBackClick(object sender, RoutedEventArgs e)
    {
        if (_voicemail is { } voicemail)
        {
            await Services.Calls.StartCallAsync(voicemail.From.Id);
        }
    }

    private async void OnDeleteClick(object sender, RoutedEventArgs e)
    {
        if (_voicemail is not { } voicemail)
        {
            return;
        }
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Delete this voicemail?",
            Content = $"The message from {voicemail.From.Label} and its transcript are deleted for good.",
            PrimaryButtonText = "Delete",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            Style = (Style)Application.Current.Resources["DefaultContentDialogStyle"],
        }.WithAppTheme();
        if (await dialog.ShowAsync() != ContentDialogResult.Primary)
        {
            return;
        }
        try
        {
            await Services.Api.DeleteVoicemailAsync(voicemail.VoicemailId);
            Services.NotifyVoicemailsChanged();
            if (Frame.CanGoBack)
            {
                Frame.GoBack();
            }
            else
            {
                App.MainWindow?.Shell.Show("voicemail");
            }
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
        }
    }
}
