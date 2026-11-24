using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Ring.Interop;
using Ring.Services;
using Windows.Graphics;
using Windows.UI;

namespace Ring.Views;

public sealed partial class MainWindow : Window
{
    private const int WidthDip = 1040;
    private const int HeightDip = 720;

    // The recording notice last shown, to bring the window forward only when a new one appears.
    private RecordingNoticeRequest? _notice;

    public MainWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(TitleBarArea);
        AppWindow.Title = "Ring";
        try
        {
            AppWindow.SetIcon(AppPaths.Asset("Ring.ico"));
        }
        catch (Exception)
        {
            // The icon is cosmetic.
        }
        AppWindow.Closing += OnClosing;
        RootControl.ActualThemeChanged += (_, _) => UpdateCaptionButtons();
        CenterOnScreen();

        Services.SessionChanged += UpdateRoot;
        Services.Calls.ActiveChanged += UpdateCall;
        Services.Calls.NoticeChanged += OnNoticeChanged;
        ApplyTheme();
        UpdateRoot();
        UpdateCall();
    }

    public ShellView Shell => ShellHost;

    public nint Handle => WinRT.Interop.WindowNative.GetWindowHandle(this);

    private static AppServices Services => App.Services;

    /// <summary>Shows, restores and focuses the window.</summary>
    public void ShowAndActivate()
    {
        if (AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Minimized } presenter)
        {
            presenter.Restore();
        }
        AppWindow.Show();
        Activate();
        Win32.SetForegroundWindow(Handle);
    }

    public void ApplyTheme()
    {
        RootControl.RequestedTheme = Services.Preferences.Theme switch
        {
            "light" => ElementTheme.Light,
            "dark" => ElementTheme.Dark,
            _ => ElementTheme.Default,
        };
        UpdateCaptionButtons();
    }

    private void UpdateCaptionButtons()
    {
        var dark = RootControl.ActualTheme == ElementTheme.Dark;
        var titleBar = AppWindow.TitleBar;
        var ink = dark ? Color.FromArgb(0xFF, 0xED, 0xE8, 0xE0) : Color.FromArgb(0xFF, 0x18, 0x18, 0x18);
        var hover = dark ? Color.FromArgb(0x24, 0xF6, 0xF2, 0xEA) : Color.FromArgb(0x14, 0x18, 0x18, 0x18);
        titleBar.ButtonBackgroundColor = Microsoft.UI.Colors.Transparent;
        titleBar.ButtonInactiveBackgroundColor = Microsoft.UI.Colors.Transparent;
        titleBar.ButtonForegroundColor = ink;
        titleBar.ButtonInactiveForegroundColor = Color.FromArgb(0x99, ink.R, ink.G, ink.B);
        titleBar.ButtonHoverBackgroundColor = hover;
        titleBar.ButtonHoverForegroundColor = ink;
        titleBar.ButtonPressedBackgroundColor = hover;
        titleBar.ButtonPressedForegroundColor = ink;
    }

    private void CenterOnScreen()
    {
        var scale = Math.Max(1.0, Win32.GetDpiForWindow(Handle) / 96.0);
        var width = (int)(WidthDip * scale);
        var height = (int)(HeightDip * scale);
        var area = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary).WorkArea;
        width = Math.Min(width, area.Width);
        height = Math.Min(height, area.Height);
        AppWindow.MoveAndResize(new RectInt32(area.X + (area.Width - width) / 2, area.Y + (area.Height - height) / 2, width, height));
    }

    private void UpdateRoot()
    {
        var signedIn = Services.IsSignedIn;
        SignInHost.Visibility = signedIn ? Visibility.Collapsed : Visibility.Visible;
        ShellHost.Visibility = signedIn ? Visibility.Visible : Visibility.Collapsed;
        if (!signedIn)
        {
            CallHost.Visibility = Visibility.Collapsed;
        }
        UpdateNotice();
    }

    private void UpdateCall()
    {
        var active = Services.Calls.Active;
        CallHost.Show(active);
        CallHost.Visibility = active is null ? Visibility.Collapsed : Visibility.Visible;
        UpdateNotice();
        if (active is not null)
        {
            ShowAndActivate();
        }
    }

    private void OnNoticeChanged()
    {
        var notice = Services.Calls.Notice;
        var appeared = notice is not null && !ReferenceEquals(notice, _notice);
        _notice = notice;
        UpdateNotice();
        // A call waits for it: bring Ring forward. The one after sign-in waits for the window.
        if (appeared && Services.Calls.NoticePlacement is not null && notice is { Action: not RecordingNoticeAction.Welcome })
        {
            ShowAndActivate();
        }
    }

    /// <summary>The recording notice over the window, or over the live call it is about.</summary>
    private void UpdateNotice()
    {
        var calls = Services.Calls;
        var placement = calls.NoticePlacement;
        var overWindow = placement == RecordingNoticePlacement.Window;
        var wasShown = NoticeHost.Visibility == Visibility.Visible || CallHost.IsShowingNotice;
        NoticeHost.Show(overWindow ? calls.Notice : null);
        CallHost.ShowNotice(placement == RecordingNoticePlacement.CallScreen ? calls.Notice : null);
        if (placement is not null && !wasShown)
        {
            CloseDialogs();
        }
    }

    /// <summary>Dialogs sit above everything in the window; make way for the recording notice.</summary>
    private void CloseDialogs()
    {
        if (RootControl.XamlRoot is not { } root)
        {
            return;
        }
        foreach (var popup in VisualTreeHelper.GetOpenPopupsForXamlRoot(root))
        {
            if (popup.Child is ContentDialog dialog)
            {
                dialog.Hide();
            }
        }
    }

    private void OnClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        if (App.IsQuitting)
        {
            return;
        }
        // Closing keeps Ring listening in the tray, as long as the tray is on (or a call is live).
        if (Services.Preferences.KeepRunningInTray || Services.Calls.Active is not null)
        {
            args.Cancel = true;
            AppWindow.Hide();
            return;
        }
        args.Cancel = true;
        App.Quit();
    }
}
