using System.Diagnostics;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Ring.Services;

namespace Ring.Views;

/// <summary>
/// After sign-in, and before a call until this device agrees: Ring records and transcribes calls,
/// and which providers process the audio. "Agree and continue" carries on with the call; "Not now"
/// drops it. Over a call that connected before this device agreed (<see cref="RecordingNoticePlacement.CallScreen"/>),
/// with its microphone muted, it covers the people and the transcript and leaves end and the audio
/// devices below it usable; "Agree and continue" turns the microphone on and "Not now" ends the call.
/// </summary>
public sealed partial class RecordingNoticeView : UserControl
{
    private RecordingNoticeRequest? _request;

    public RecordingNoticeView()
    {
        InitializeComponent();
        IsEnabled = false;
    }

    /// <summary>Over the screen of the live call it is about, rather than over the whole window.</summary>
    public RecordingNoticePlacement Placement { get; set; } = RecordingNoticePlacement.Window;

    private static AppServices Services => App.Services;

    /// <summary>Shows <paramref name="request"/>, or hides the notice when null.</summary>
    public void Show(RecordingNoticeRequest? request)
    {
        var appearing = request is not null && !ReferenceEquals(request, _request);
        _request = request;
        // Collapsed and disabled, its Enter and Esc accelerators stay out of the way.
        Visibility = request is null ? Visibility.Collapsed : Visibility.Visible;
        IsEnabled = request is not null;
        if (request is null)
        {
            return;
        }

        var context = Context(request);
        ContextText.Text = context ?? "";
        ContextText.Visibility = context is null ? Visibility.Collapsed : Visibility.Visible;
        AutomationProperties.SetHelpText(NotNowButton, NotNowHint(request));
        // Over a call, Tab still reaches the call controls below it.
        Scrim.TabFocusNavigation = Placement == RecordingNoticePlacement.CallScreen
            ? KeyboardNavigationMode.Local
            : KeyboardNavigationMode.Cycle;
        if (appearing)
        {
            // After layout, once the button can take focus: Enter agrees, Esc is Not now.
            DispatcherQueue.TryEnqueue(() =>
            {
                if (ReferenceEquals(_request, request))
                {
                    AgreeButton.Focus(FocusState.Programmatic);
                }
            });
        }
    }

    /// <summary>For a call that connected before this device agreed.</summary>
    private string? Context(RecordingNoticeRequest request)
    {
        if (!request.IsAnswered)
        {
            return null;
        }
        return Placement == RecordingNoticePlacement.CallScreen
            ? "This call is recorded and transcribed, like every Ring call. Your microphone stays off until you agree; Not now ends the call."
            : "Your last call was recorded and transcribed, like every Ring call.";
    }

    private string NotNowHint(RecordingNoticeRequest request) => request.Action switch
    {
        RecordingNoticeAction.Welcome => "Asks again before your first call",
        RecordingNoticeAction.Place => "Doesn't place the call",
        RecordingNoticeAction.Answer => "Doesn't answer the call",
        RecordingNoticeAction.SwitchHere => "Leaves the call on your other device",
        _ => Placement == RecordingNoticePlacement.CallScreen ? "Ends the call" : "Closes the notice",
    };

    private void OnAgreeClick(object sender, RoutedEventArgs e) => Services.Calls.AgreeToRecordingNotice();

    private void OnNotNowClick(object sender, RoutedEventArgs e) => Services.Calls.DismissRecordingNotice();

    private void OnPrivacyClick(object sender, RoutedEventArgs e)
    {
        try
        {
            Process.Start(new ProcessStartInfo(AppServices.PrivacyUrl) { UseShellExecute = true });
        }
        catch (Exception ex) when (ex is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            // No browser to open it; the notice itself says what matters.
        }
    }
}
