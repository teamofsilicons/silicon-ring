using System.Collections.ObjectModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Ring.Api;
using Ring.Services;
using Ring.ViewModels;
using Windows.System;

namespace Ring.Views;

/// <summary>The dialer: type an id for a live lookup, or pick someone recent.</summary>
public sealed partial class DialerView : UserControl
{
    private readonly ObservableCollection<PersonItem> _recent = [];
    private CancellationTokenSource? _lookup;
    private AccountLookup? _result;
    private bool _invite;

    public DialerView()
    {
        InitializeComponent();
        RecentList.ItemsSource = _recent;
        Loaded += async (_, _) =>
        {
            QueryBox.Focus(FocusState.Programmatic);
            await LoadRecentAsync();
        };
    }

    /// <summary>Adding someone to the current call rather than starting a new one.</summary>
    public bool Invite
    {
        get => _invite;
        set
        {
            _invite = value;
            ResultCallLabel.Text = value ? "Add" : "Call";
        }
    }

    /// <summary>The actor reference to call or invite.</summary>
    public event Action<string>? Picked;

    private static AppServices Services => App.Services;

    private async Task LoadRecentAsync()
    {
        try
        {
            var people = await Services.Api.GetRecentPeopleAsync(limit: 8);
            _recent.Clear();
            foreach (var person in people)
            {
                var meta = $"{Format.Pixel(person.Account.Id)} · {Format.When(person.LastCallAt)}";
                _recent.Add(new PersonItem(person.Account, meta));
            }
        }
        catch (ApiException)
        {
            // The list is a shortcut; typing an id still works.
        }
        RecentHeader.Visibility = _recent.Count == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private async void OnQueryChanged(object sender, TextChangedEventArgs e)
    {
        _lookup?.Cancel();
        _result = null;
        ResultCard.Visibility = Visibility.Collapsed;
        var text = QueryBox.Text;
        if (string.IsNullOrWhiteSpace(text))
        {
            StatusText.Visibility = Visibility.Collapsed;
            return;
        }
        var reference = Format.ActorReference(text);
        if (reference is null)
        {
            SetStatus(text.Contains(':') ? "THAT DOESN'T LOOK LIKE AN ID" : "START WITH C: OR SI:");
            return;
        }

        var cts = new CancellationTokenSource();
        _lookup = cts;
        SetStatus("LOOKING UP…");
        try
        {
            await Task.Delay(300, cts.Token);
            var lookup = await Services.Api.LookupAccountAsync(reference, cts.Token);
            if (cts.IsCancellationRequested)
            {
                return;
            }
            ShowResult(lookup);
        }
        catch (OperationCanceledException)
        {
            // A newer keystroke took over.
        }
        catch (ApiException ex)
        {
            if (!cts.IsCancellationRequested)
            {
                SetStatus(ex.Code == "account_not_found" ? "NO ONE HAS THAT ID" : Format.Pixel(ex.Message));
            }
        }
    }

    private void ShowResult(AccountLookup lookup)
    {
        _result = lookup;
        StatusText.Visibility = Visibility.Collapsed;
        ResultAvatar.Account = lookup.Account;
        ResultName.Text = lookup.Account.Label;
        ResultHandle.Text = Format.Pixel(lookup.Account.Id);
        SiliconBadge.Visibility = lookup.Account.IsSilicon ? Visibility.Visible : Visibility.Collapsed;
        OnRingText.Text = lookup.OnRing ? "ON RING" : "NOT ON RING YET";
        ResultCard.Visibility = Visibility.Visible;
    }

    private void SetStatus(string text)
    {
        StatusText.Text = text;
        StatusText.Visibility = Visibility.Visible;
    }

    private void OnResultCallClick(object sender, RoutedEventArgs e)
    {
        if (_result is { } result)
        {
            Picked?.Invoke(result.Account.Id);
        }
    }

    private void OnQueryKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter && _result is { } result)
        {
            e.Handled = true;
            Picked?.Invoke(result.Account.Id);
        }
    }

    private void OnRecentClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is PersonItem person)
        {
            Picked?.Invoke(person.Account.Id);
        }
    }
}

/// <summary>Shows the dialer in a dialog and returns the chosen actor reference, or null.</summary>
public static class DialerDialog
{
    public static async Task<string?> PickAsync(XamlRoot xamlRoot, bool invite)
    {
        var view = new DialerView { Invite = invite };
        var dialog = new ContentDialog
        {
            XamlRoot = xamlRoot,
            Title = invite ? "Add to call" : "New call",
            Content = view,
            CloseButtonText = "Cancel",
            Style = (Style)Application.Current.Resources["DefaultContentDialogStyle"],
        }.WithAppTheme();
        string? picked = null;
        view.Picked += id =>
        {
            picked = id;
            dialog.Hide();
        };
        try
        {
            await dialog.ShowAsync();
        }
        catch (System.Runtime.InteropServices.COMException)
        {
            // Another dialog is already open on this window.
            return null;
        }
        return picked;
    }
}
