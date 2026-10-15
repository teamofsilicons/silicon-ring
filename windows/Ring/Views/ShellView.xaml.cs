using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Navigation;
using Ring.Api;
using Ring.Services;

namespace Ring.Views;

/// <summary>Signed-in home: sidebar (Recents, Voicemail, Settings), header and page frame.</summary>
public sealed partial class ShellView : UserControl
{
    private bool _attached;

    public ShellView()
    {
        InitializeComponent();
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    private static AppServices Services => App.Services;

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        if (_attached)
        {
            return;
        }
        _attached = true;
        Services.Notice += OnNotice;
        Services.ConnectionChanged += UpdateConnection;
        Services.VoicemailsChanged += OnVoicemailsChanged;
        Services.Calls.ElsewhereChanged += UpdateElsewhere;
        Services.SessionChanged += OnSessionChanged;
        UpdateConnection();
        UpdateElsewhere();
        if (ContentFrame.Content is null && Services.IsSignedIn)
        {
            Nav.SelectedItem = RecentsItem;
            _ = RefreshVoicemailBadgeAsync();
        }
    }

    private void OnUnloaded(object sender, RoutedEventArgs e)
    {
        if (!_attached)
        {
            return;
        }
        _attached = false;
        Services.Notice -= OnNotice;
        Services.ConnectionChanged -= UpdateConnection;
        Services.VoicemailsChanged -= OnVoicemailsChanged;
        Services.Calls.ElsewhereChanged -= UpdateElsewhere;
        Services.SessionChanged -= OnSessionChanged;
    }

    private void OnSessionChanged()
    {
        if (!Services.IsSignedIn)
        {
            return;
        }
        NoticeBar.IsOpen = false;
        if (ReferenceEquals(Nav.SelectedItem, RecentsItem))
        {
            NavigateTo("recents");
        }
        else
        {
            Nav.SelectedItem = RecentsItem;
        }
        _ = RefreshVoicemailBadgeAsync();
    }

    /// <summary>Opens a top-level section: recents, voicemail or settings.</summary>
    public void Show(string section)
    {
        var item = section switch
        {
            "voicemail" => VoicemailItem,
            "settings" => SettingsItem,
            _ => RecentsItem,
        };
        if (ReferenceEquals(Nav.SelectedItem, item))
        {
            NavigateTo(section);
        }
        else
        {
            Nav.SelectedItem = item;
        }
    }

    public void ShowCallDetails(string ringid) =>
        ContentFrame.Navigate(typeof(CallDetailsPage), ringid, new SlideNavigationTransitionInfo { Effect = SlideNavigationTransitionEffect.FromRight });

    public void ShowVoicemail(string voicemailId) =>
        ContentFrame.Navigate(typeof(VoicemailDetailPage), voicemailId, new SlideNavigationTransitionInfo { Effect = SlideNavigationTransitionEffect.FromRight });

    private void OnNavSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is NavigationViewItem { Tag: string tag })
        {
            NavigateTo(tag);
        }
    }

    private void NavigateTo(string tag)
    {
        var page = tag switch
        {
            "voicemail" => typeof(VoicemailPage),
            "settings" => typeof(SettingsPage),
            _ => typeof(RecentsPage),
        };
        ContentFrame.Navigate(page, null, new EntranceNavigationTransitionInfo());
        ContentFrame.BackStack.Clear();
        BackButton.Visibility = Visibility.Collapsed;
    }

    private void OnNavigated(object sender, NavigationEventArgs e)
    {
        PageTitle.Text = e.SourcePageType == typeof(VoicemailPage) ? "Voicemail"
            : e.SourcePageType == typeof(SettingsPage) ? "Settings"
            : e.SourcePageType == typeof(CallDetailsPage) ? "Call details"
            : e.SourcePageType == typeof(VoicemailDetailPage) ? "Voicemail"
            : "Recents";
        BackButton.Visibility = ContentFrame.CanGoBack ? Visibility.Visible : Visibility.Collapsed;
    }

    private void OnBackClick(object sender, RoutedEventArgs e)
    {
        if (ContentFrame.CanGoBack)
        {
            ContentFrame.GoBack();
        }
    }

    private async void OnNewCallClick(object sender, RoutedEventArgs e)
    {
        var target = await DialerDialog.PickAsync(XamlRoot, invite: false);
        if (target is not null)
        {
            await Services.Calls.StartCallAsync(target);
        }
    }

    private async void OnSwitchHereClick(object sender, RoutedEventArgs e)
    {
        if (Services.Calls.Elsewhere is { } elsewhere)
        {
            await Services.Calls.SwitchHereAsync(elsewhere.Ringid);
        }
    }

    private void UpdateElsewhere()
    {
        var elsewhere = Services.Calls.Elsewhere;
        ElsewhereBanner.Visibility = elsewhere is null ? Visibility.Collapsed : Visibility.Visible;
        ElsewhereText.Text = elsewhere?.Message ?? "";
    }

    private void UpdateConnection()
    {
        if (Services.Connection == ConnectionStatus.Offline)
        {
            NoticeBar.Severity = InfoBarSeverity.Warning;
            NoticeBar.Title = "Offline";
            NoticeBar.Message = "Ring is reconnecting. Calls can't reach this PC until it's back.";
            NoticeBar.IsClosable = false;
            NoticeBar.IsOpen = true;
        }
        else if (NoticeBar.IsOpen && !NoticeBar.IsClosable)
        {
            NoticeBar.IsOpen = false;
        }
    }

    private void OnNotice(string message, bool isError)
    {
        NoticeBar.Severity = isError ? InfoBarSeverity.Error : InfoBarSeverity.Informational;
        NoticeBar.Title = "";
        NoticeBar.Message = message;
        NoticeBar.IsClosable = true;
        NoticeBar.IsOpen = true;
    }

    private void OnVoicemailsChanged() => _ = RefreshVoicemailBadgeAsync();

    private async Task RefreshVoicemailBadgeAsync()
    {
        if (!Services.IsSignedIn)
        {
            return;
        }
        try
        {
            var unread = await Services.Api.ListVoicemailsAsync(unreadOnly: true, limit: 100);
            var count = unread.Items.Count;
            VoicemailBadge.Value = count;
            VoicemailBadge.Visibility = count > 0 ? Visibility.Visible : Visibility.Collapsed;
        }
        catch (ApiException)
        {
            // Keep the last count.
        }
    }
}
