using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Ring.Interop;
using Ring.Services;
using Windows.Graphics;
using Windows.UI;

namespace Ring.Views;

public sealed partial class MainWindow : Window
{
    private const int WidthDip = 1040;
    private const int HeightDip = 720;

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
    }

    private void UpdateCall()
    {
        var active = Services.Calls.Active;
        CallHost.Show(active);
        CallHost.Visibility = active is null ? Visibility.Collapsed : Visibility.Visible;
        if (active is not null)
        {
            ShowAndActivate();
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
