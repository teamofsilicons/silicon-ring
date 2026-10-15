using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Ring.Interop;
using Ring.Services;
using Ring.ViewModels;
using Windows.Graphics;
using Windows.System;

namespace Ring.Views;

/// <summary>
/// The always-on-top ringing window, bottom right of the primary display. It shows without
/// taking focus so typing elsewhere can't answer or decline by accident.
/// </summary>
public sealed partial class IncomingCallWindow : Window
{
    private const int WidthDip = 360;
    private const int HeightDip = 600;

    private IncomingCall? _incoming;
    private bool _reasonsShown;
    private bool _busy;
    private bool _closed;

    public IncomingCallWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        AppWindow.Title = "Incoming call";
        try
        {
            AppWindow.SetIcon(AppPaths.Asset("Ring.ico"));
        }
        catch (Exception)
        {
            // The icon is cosmetic.
        }
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsAlwaysOnTop = true;
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
            presenter.SetBorderAndTitleBar(true, false);
        }
        var buttons = new[] { Reason0, Reason1, Reason2, Reason3 };
        for (var i = 0; i < buttons.Length; i++)
        {
            buttons[i].Content = CallCoordinator.DeclineReasons[i];
            buttons[i].Tag = CallCoordinator.DeclineReasons[i];
        }
        // Alt+F4 can close it before the call stops ringing.
        Closed += (_, _) => _closed = true;
    }

    public IncomingCall? Incoming => _incoming;

    /// <summary>Closes the window unless the user already closed it.</summary>
    public void CloseIfOpen()
    {
        if (!_closed)
        {
            _closed = true;
            Close();
        }
    }

    private static AppServices Services => App.Services;

    public void ShowFor(IncomingCall incoming)
    {
        _incoming = incoming;
        var caller = incoming.Caller;
        CallerAvatar.Account = caller;
        CallerName.Text = caller.Label;
        CallerHandle.Text = caller.Id;
        // Without a display name the id is already the large line.
        CallerHandle.Visibility = caller.HasDisplayName ? Visibility.Visible : Visibility.Collapsed;
        SiliconBadge.Visibility = caller.IsSilicon ? Visibility.Visible : Visibility.Collapsed;
        var also = incoming.AlsoOnCall;
        AlsoText.Text = also is null ? "" : $"WITH {also.ToUpperInvariant()}";
        AlsoText.Visibility = also is null ? Visibility.Collapsed : Visibility.Visible;
        AcceptLabel.Text = Services.Calls.Active is null ? "ACCEPT" : "END & ACCEPT";

        PlaceBottomRight();
        AppWindow.Show(false);
    }

    private void PlaceBottomRight()
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        var scale = Math.Max(1.0, Win32.GetDpiForWindow(hwnd) / 96.0);
        var width = (int)(WidthDip * scale);
        var height = (int)(HeightDip * scale);
        var margin = (int)(16 * scale);
        var area = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary).WorkArea;
        AppWindow.MoveAndResize(new RectInt32(
            area.X + area.Width - width - margin,
            area.Y + area.Height - height - margin,
            width,
            height));
    }

    private async void OnAcceptClick(object sender, RoutedEventArgs e)
    {
        if (_incoming is null || _busy)
        {
            return;
        }
        _busy = true;
        await Services.Calls.AcceptAsync(_incoming);
        App.ShowMainWindow();
    }

    private async void OnDeclineClick(object sender, RoutedEventArgs e)
    {
        if (_incoming is null || _busy)
        {
            return;
        }
        if (!_reasonsShown)
        {
            _reasonsShown = true;
            ReasonsPanel.Visibility = Visibility.Visible;
            DeclineLabel.Text = "NO REASON";
            return;
        }
        var typed = ReasonBox.Text.Trim();
        await DeclineAsync(typed.Length == 0 ? null : typed);
    }

    private async void OnReasonClick(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: string reason })
        {
            await DeclineAsync(reason);
        }
    }

    // A typed reason goes with the second press of Decline; without one it declines with none.
    private void OnReasonTextChanged(object sender, TextChangedEventArgs e) =>
        DeclineLabel.Text = ReasonBox.Text.Trim().Length > 0 ? "DECLINE" : "NO REASON";

    private async void OnReasonKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter && ReasonBox.Text.Trim().Length > 0)
        {
            e.Handled = true;
            await DeclineAsync(ReasonBox.Text.Trim());
        }
    }

    private async Task DeclineAsync(string? reason)
    {
        if (_incoming is null || _busy)
        {
            return;
        }
        _busy = true;
        await Services.Calls.DeclineAsync(_incoming, reason);
    }

    private async void OnSilenceClick(object sender, RoutedEventArgs e)
    {
        if (_incoming is null || _busy)
        {
            return;
        }
        _busy = true;
        await Services.Calls.SilenceAsync(_incoming);
    }
}
