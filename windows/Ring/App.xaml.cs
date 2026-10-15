using System.Net.NetworkInformation;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using Microsoft.Windows.AppNotifications;
using Ring.Services;
using Ring.ViewModels;
using Ring.Views;

namespace Ring;

public partial class App : Application
{
    private static AppServices? s_services;
    private static TrayIcon? s_tray;
    private static readonly Dictionary<string, IncomingCallWindow> s_ringing = [];

    public App()
    {
        InitializeComponent();
        // Ring is a background listener; one failed handler must not end it.
        UnhandledException += (_, e) =>
        {
            LocalLog.Error("unhandled", e.Exception);
            e.Handled = true;
        };
    }

    public static AppServices Services => s_services ?? throw new InvalidOperationException("Ring has not started yet.");

    public static MainWindow? MainWindow { get; private set; }

    public static bool IsQuitting { get; private set; }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        // Ring keeps running with every window closed: the tray and the socket listen for calls.
        DispatcherShutdownMode = DispatcherShutdownMode.OnExplicitShutdown;

        var services = new AppServices(DispatcherQueue.GetForCurrentThread());
        s_services = services;

        services.Notifications.Invoked += action => services.Post(() => HandleNotification(action));
        services.Notifications.Register();

        services.Calls.IncomingAdded += OnIncomingAdded;
        services.Calls.IncomingRemoved += OnIncomingRemoved;
        services.SessionChanged += UpdateTray;
        services.ConnectionChanged += UpdateTrayTooltip;

        AppInstance.GetCurrent().Activated += (_, e) => services.Post(() => OnRedirected(e));
        NetworkChange.NetworkAddressChanged += (_, _) => services.Post(services.ReconnectSoon);

        MainWindow = new MainWindow();
        services.Restore();
        try
        {
            StartupRegistration.Refresh();
        }
        catch (Exception)
        {
            // Keeping the Run entry pointed at this build is best effort.
        }
        UpdateTray();

        var background = Environment.GetCommandLineArgs().Contains(StartupRegistration.BackgroundArgument);
        if (!background || !services.IsSignedIn || !services.Preferences.KeepRunningInTray)
        {
            MainWindow.ShowAndActivate();
        }

        var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
        if (activation.Kind == ExtendedActivationKind.AppNotification && activation.Data is AppNotificationActivatedEventArgs launched)
        {
            HandleNotification(CallNotifications.ParseArguments(launched.Argument) ?? new NotificationAction("open", null, null, null, null));
        }
    }

    /// <summary>Another launch of Ring.exe (Start menu, a second click) was redirected here.</summary>
    private static void OnRedirected(AppActivationArguments args)
    {
        if (args.Kind == ExtendedActivationKind.AppNotification && args.Data is AppNotificationActivatedEventArgs notification)
        {
            HandleNotification(CallNotifications.ParseArguments(notification.Argument) ?? new NotificationAction("open", null, null, null, null));
            return;
        }
        ShowMainWindow();
    }

    private static async void HandleNotification(NotificationAction action)
    {
        var services = Services;
        switch (action.Action)
        {
            case "answer" when action.Ringid is not null:
                await services.Calls.AcceptByIdAsync(action.Ringid, action.InvitationId);
                ShowMainWindow();
                break;
            case "decline" when action.Ringid is not null:
                await services.Calls.DeclineByIdAsync(action.Ringid, action.InvitationId);
                break;
            case "callback" when action.To is not null:
                ShowMainWindow();
                await services.Calls.StartCallAsync(action.To);
                break;
            case "voicemail" when action.VoicemailId is not null:
                ShowMainWindow();
                MainWindow?.Shell.ShowVoicemail(action.VoicemailId);
                break;
            default:
                ShowMainWindow();
                if (action.Ringid is not null && services.Calls.FindIncoming(action.Ringid, action.InvitationId) is null)
                {
                    MainWindow?.Shell.ShowCallDetails(action.Ringid);
                }
                break;
        }
    }

    public static void ShowMainWindow()
    {
        MainWindow ??= new MainWindow();
        MainWindow.ShowAndActivate();
    }

    public static void ApplyTheme() => MainWindow?.ApplyTheme();

    // ---- ringing windows --------------------------------------------------------------------------

    private static void OnIncomingAdded(IncomingCall incoming)
    {
        if (s_ringing.ContainsKey(incoming.InvitationId))
        {
            return;
        }
        var window = new IncomingCallWindow();
        s_ringing[incoming.InvitationId] = window;
        window.ShowFor(incoming);
    }

    private static void OnIncomingRemoved(IncomingCall incoming)
    {
        if (s_ringing.Remove(incoming.InvitationId, out var window))
        {
            window.CloseIfOpen();
        }
    }

    // ---- tray -------------------------------------------------------------------------------------

    public static void UpdateTray()
    {
        var services = Services;
        var wanted = services.Preferences.KeepRunningInTray;
        if (wanted && s_tray is null)
        {
            s_tray = new TrayIcon();
            s_tray.OpenRequested += () => services.Post(ShowMainWindow);
            s_tray.QuitRequested += () => services.Post(Quit);
        }
        if (s_tray is null)
        {
            return;
        }
        if (wanted)
        {
            s_tray.Show();
            UpdateTrayTooltip();
        }
        else
        {
            s_tray.Hide();
        }
    }

    private static void UpdateTrayTooltip()
    {
        if (s_tray is null || s_services is null)
        {
            return;
        }
        s_tray.SetTooltip(s_services.Connection switch
        {
            ConnectionStatus.SignedOut => "Ring · signed out",
            ConnectionStatus.Offline => "Ring · offline, reconnecting",
            ConnectionStatus.Connecting => "Ring · connecting",
            _ => "Ring · listening for calls",
        });
    }

    // ---- shutdown ---------------------------------------------------------------------------------

    public static async void Quit()
    {
        if (IsQuitting)
        {
            return;
        }
        IsQuitting = true;
        var services = Services;
        if (services.Calls.Active is not null)
        {
            await services.Calls.HangUpAsync();
        }
        await services.ShutdownAsync();
        foreach (var window in s_ringing.Values.ToList())
        {
            window.CloseIfOpen();
        }
        s_ringing.Clear();
        s_tray?.Dispose();
        s_tray = null;
        MainWindow?.Close();
        Current.Exit();
    }
}
