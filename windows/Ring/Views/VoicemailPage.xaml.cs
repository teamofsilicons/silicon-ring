using System.Collections.ObjectModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Ring.Api;
using Ring.Services;
using Ring.ViewModels;
using Windows.Media.Core;
using Windows.Media.Playback;

namespace Ring.Views;

public sealed partial class VoicemailPage : Page
{
    private readonly ObservableCollection<VoicemailItem> _items = [];
    private readonly MediaPlayer _player = new();
    private VoicemailItem? _playing;
    private bool _loading;
    private bool _disposed;

    public VoicemailPage()
    {
        InitializeComponent();
        VoicemailList.ItemsSource = _items;
        _player.MediaEnded += (_, _) => Services.Post(StopPlayback);
        _player.MediaFailed += (_, _) => Services.Post(StopPlayback);
    }

    private static AppServices Services => App.Services;

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        Services.VoicemailsChanged += OnVoicemailsChanged;
        _ = LoadAsync();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        Services.VoicemailsChanged -= OnVoicemailsChanged;
        StopPlayback();
        _disposed = true;
        _player.Dispose();
    }

    private void OnVoicemailsChanged() => _ = LoadAsync();

    private async Task LoadAsync()
    {
        if (_loading)
        {
            return;
        }
        _loading = true;
        LoadingRing.IsActive = _items.Count == 0;
        try
        {
            var page = await Services.Api.ListVoicemailsAsync(limit: 100);
            var byId = _items.ToDictionary(i => i.Id);
            var fresh = new List<VoicemailItem>();
            foreach (var voicemail in page.Items)
            {
                if (byId.TryGetValue(voicemail.VoicemailId, out var existing))
                {
                    existing.Voicemail = voicemail;
                    fresh.Add(existing);
                }
                else
                {
                    fresh.Add(new VoicemailItem(voicemail));
                }
            }
            _items.Clear();
            foreach (var item in fresh)
            {
                _items.Add(item);
            }
        }
        catch (ApiException ex) when (ex.Code != "network")
        {
            Services.ShowNotice(ex.UserMessage);
        }
        catch (ApiException)
        {
            // Offline.
        }
        finally
        {
            _loading = false;
            LoadingRing.IsActive = false;
            ListCard.Visibility = _items.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            EmptyState.Visibility = _items.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        }
    }

    private void OnVoicemailClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is VoicemailItem item)
        {
            StopPlayback();
            App.MainWindow?.Shell.ShowVoicemail(item.Id);
        }
    }

    private async void OnPlayClick(object sender, RoutedEventArgs e)
    {
        if (sender is not FrameworkElement { Tag: VoicemailItem item })
        {
            return;
        }
        if (_playing == item)
        {
            StopPlayback();
            return;
        }
        StopPlayback();
        try
        {
            // The list omits audio URLs; the detail call presigns one for ten minutes.
            var voicemail = await Services.Api.GetVoicemailAsync(item.Id);
            if (_disposed)
            {
                return;
            }
            if (string.IsNullOrEmpty(voicemail.AudioUrl))
            {
                Services.ShowNotice("This voicemail's audio isn't ready yet.", isError: false);
                return;
            }
            _player.Source = MediaSource.CreateFromUri(new Uri(voicemail.AudioUrl));
            _player.Play();
            _playing = item;
            item.IsPlaying = true;
            if (!voicemail.Read)
            {
                item.Voicemail = await Services.Api.SetVoicemailReadAsync(item.Id, true);
                Services.NotifyVoicemailsChanged();
            }
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
        }
    }

    private void StopPlayback()
    {
        // MediaEnded can still be queued after the page closed its player.
        if (_disposed)
        {
            return;
        }
        _player.Pause();
        _player.Source = null;
        if (_playing is not null)
        {
            _playing.IsPlaying = false;
            _playing = null;
        }
    }
}
