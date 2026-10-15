using System.Collections.ObjectModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Ring.Api;
using Ring.Services;
using Ring.ViewModels;

namespace Ring.Views;

public sealed partial class RecentsPage : Page
{
    private readonly ObservableCollection<RecentCallItem> _items = [];
    private string? _cursor;
    private bool _loading;
    private bool _stale;

    public RecentsPage()
    {
        InitializeComponent();
        CallsList.ItemsSource = _items;
    }

    private static AppServices Services => App.Services;

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        Services.Calls.HistoryChanged += OnHistoryChanged;
        _ = LoadAsync(reset: true);
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        Services.Calls.HistoryChanged -= OnHistoryChanged;
    }

    private void OnHistoryChanged() => _ = LoadAsync(reset: true);

    private async Task LoadAsync(bool reset)
    {
        if (_loading)
        {
            _stale = true;
            return;
        }
        _loading = true;
        LoadingRing.IsActive = _items.Count == 0;
        try
        {
            var page = await Services.Api.ListCallsAsync(new CallQuery(Direction: "all", Limit: 50, Cursor: reset ? null : _cursor));
            var me = Services.Me?.Uuid ?? "";
            if (reset)
            {
                _items.Clear();
            }
            foreach (var call in page.Items)
            {
                _items.Add(new RecentCallItem(call, me));
            }
            _cursor = page.NextCursor;
        }
        catch (ApiException ex) when (ex.Code != "network")
        {
            Services.ShowNotice(ex.UserMessage);
        }
        catch (ApiException)
        {
            // Offline: keep what is on screen; the reconnect refreshes it.
        }
        finally
        {
            _loading = false;
            LoadingRing.IsActive = false;
            ListCard.Visibility = _items.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            EmptyState.Visibility = _items.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
            MoreButton.Visibility = _cursor is null ? Visibility.Collapsed : Visibility.Visible;
        }
        if (_stale)
        {
            _stale = false;
            await LoadAsync(reset: true);
        }
    }

    private void OnCallClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is RecentCallItem item)
        {
            App.MainWindow?.Shell.ShowCallDetails(item.Ringid);
        }
    }

    private async void OnRedialClick(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: string target })
        {
            await Services.Calls.StartCallAsync(target);
        }
    }

    private void OnMoreClick(object sender, RoutedEventArgs e) => _ = LoadAsync(reset: false);
}
