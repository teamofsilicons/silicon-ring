using System.Runtime.InteropServices;
using Ring.Interop;

namespace Ring.Services;

/// <summary>
/// The notification-area icon (Shell_NotifyIcon). It lives on the UI thread with its own hidden
/// window, so the app keeps listening for calls after the main window closes.
/// </summary>
public sealed unsafe class TrayIcon : IDisposable
{
    private const uint CallbackMessage = Win32.WM_APP + 1;
    private const uint IconId = 1;
    private const int CommandOpen = 1;
    private const int CommandQuit = 2;

    // One tray icon per process; the window procedure finds it here.
    private static TrayIcon? s_current;
    private static uint s_taskbarCreated;

    private readonly nint _window;
    private nint _icon;
    private string _tooltip = "Ring";
    private bool _added;
    // Whether the icon should be shown; at login Explorer may not be ready for the first add.
    private bool _wanted;

    public TrayIcon()
    {
        s_current = this;
        s_taskbarCreated = Win32.RegisterWindowMessage("TaskbarCreated");

        var instance = Win32.GetModuleHandle(null);
        const string className = "TeamOfSilicons.Ring.Tray";
        fixed (char* name = className)
        {
            var wc = new Win32.WNDCLASSEXW
            {
                cbSize = (uint)sizeof(Win32.WNDCLASSEXW),
                lpfnWndProc = &WindowProc,
                hInstance = instance,
                lpszClassName = (nint)name,
            };
            Win32.RegisterClassEx(&wc);
        }
        _window = Win32.CreateWindowEx(0, className, "Ring", 0, 0, 0, 0, 0, 0, 0, instance, 0);

        var iconPath = AppPaths.Asset("Ring.ico");
        _icon = Win32.LoadImage(0, iconPath, Win32.IMAGE_ICON,
            Win32.GetSystemMetrics(Win32.SM_CXSMICON), Win32.GetSystemMetrics(Win32.SM_CYSMICON), Win32.LR_LOADFROMFILE);
    }

    /// <summary>Left click on the icon, or "Open Ring" in its menu.</summary>
    public event Action? OpenRequested;

    /// <summary>"Quit Ring" in its menu.</summary>
    public event Action? QuitRequested;

    public bool IsVisible => _added;

    public void Show()
    {
        _wanted = true;
        if (_added || _window == 0)
        {
            return;
        }
        var data = NewData();
        _added = Win32.Shell_NotifyIcon(Win32.NIM_ADD, &data) != 0;
    }

    public void Hide()
    {
        _wanted = false;
        if (!_added)
        {
            return;
        }
        var data = NewData();
        Win32.Shell_NotifyIcon(Win32.NIM_DELETE, &data);
        _added = false;
    }

    public void SetTooltip(string text)
    {
        _tooltip = text;
        if (_added)
        {
            var data = NewData();
            Win32.Shell_NotifyIcon(Win32.NIM_MODIFY, &data);
        }
    }

    public void Dispose()
    {
        Hide();
        if (_icon != 0)
        {
            Win32.DestroyIcon(_icon);
            _icon = 0;
        }
        if (_window != 0)
        {
            Win32.DestroyWindow(_window);
        }
        if (s_current == this)
        {
            s_current = null;
        }
    }

    private Win32.NOTIFYICONDATAW NewData()
    {
        var data = new Win32.NOTIFYICONDATAW
        {
            cbSize = (uint)sizeof(Win32.NOTIFYICONDATAW),
            hWnd = _window,
            uID = IconId,
            uFlags = Win32.NIF_MESSAGE | Win32.NIF_ICON | Win32.NIF_TIP,
            uCallbackMessage = CallbackMessage,
            hIcon = _icon,
        };
        Win32.CopyToBuffer(_tooltip, data.szTip, 128);
        return data;
    }

    private void ShowMenu()
    {
        var menu = Win32.CreatePopupMenu();
        if (menu == 0)
        {
            return;
        }
        try
        {
            Win32.AppendMenu(menu, Win32.MF_STRING, CommandOpen, "Open Ring");
            Win32.AppendMenu(menu, Win32.MF_SEPARATOR, 0, null);
            Win32.AppendMenu(menu, Win32.MF_STRING, CommandQuit, "Quit Ring");

            Win32.POINT point;
            Win32.GetCursorPos(&point);
            // The menu only dismisses on an outside click while our window is foreground.
            Win32.SetForegroundWindow(_window);
            var command = Win32.TrackPopupMenuEx(menu, Win32.TPM_RETURNCMD | Win32.TPM_RIGHTBUTTON | Win32.TPM_NONOTIFY,
                point.X, point.Y, _window, 0);
            Win32.PostMessage(_window, Win32.WM_NULL, 0, 0);

            switch (command)
            {
                case CommandOpen:
                    OpenRequested?.Invoke();
                    break;
                case CommandQuit:
                    QuitRequested?.Invoke();
                    break;
            }
        }
        finally
        {
            Win32.DestroyMenu(menu);
        }
    }

    private void OnTaskbarCreated()
    {
        // Explorer (re)started: it forgot the icon, or was not ready when Ring first added it.
        _added = false;
        if (_wanted)
        {
            Show();
        }
    }

    [UnmanagedCallersOnly]
    private static nint WindowProc(nint hwnd, uint message, nint wParam, nint lParam)
    {
        try
        {
            var tray = s_current;
            if (tray is not null)
            {
                if (message == CallbackMessage)
                {
                    switch ((uint)(lParam & 0xFFFF))
                    {
                        case Win32.WM_LBUTTONUP:
                        case Win32.WM_LBUTTONDBLCLK:
                            tray.OpenRequested?.Invoke();
                            return 0;
                        case Win32.WM_RBUTTONUP:
                        case Win32.WM_CONTEXTMENU:
                            tray.ShowMenu();
                            return 0;
                    }
                    return 0;
                }
                if (message == s_taskbarCreated && s_taskbarCreated != 0)
                {
                    tray.OnTaskbarCreated();
                    return 0;
                }
            }
        }
        catch (Exception)
        {
            // Never let an exception unwind into user32.
        }
        return Win32.DefWindowProc(hwnd, message, wParam, lParam);
    }
}
