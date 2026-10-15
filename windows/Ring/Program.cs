using System.Diagnostics;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using Ring.Interop;
using Ring.Services;

namespace Ring;

/// <summary>
/// Entry point. Ring is single-instance: a second launch hands its activation (a Start menu click,
/// a notification) to the running instance and exits.
/// </summary>
public static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        WinRT.ComWrappersSupport.InitializeComWrappers();
        // One identity for the taskbar, notifications and the single-instance key.
        Win32.SetCurrentProcessExplicitAppUserModelID(AppServices.AppUserModelId);

        var instance = AppInstance.FindOrRegisterForKey(AppServices.AppUserModelId);
        if (!instance.IsCurrent)
        {
            Redirect(instance);
            return 0;
        }

        Application.Start(p =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            _ = new App();
        });
        return 0;
    }

    private static unsafe void Redirect(AppInstance target)
    {
        var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
        try
        {
            Win32.AllowSetForegroundWindow(target.ProcessId);
        }
        catch (Exception)
        {
            // Foreground hand-over is a nicety.
        }

        // Wait for the redirect without blocking COM on this STA thread.
        using var done = new EventWaitHandle(false, EventResetMode.ManualReset);
        _ = Task.Run(() =>
        {
            try
            {
                target.RedirectActivationToAsync(activation).AsTask().Wait();
            }
            catch (Exception ex)
            {
                Debug.WriteLine($"Ring: redirect failed: {ex.Message}");
            }
            finally
            {
                done.Set();
            }
        });
        var handle = done.SafeWaitHandle.DangerousGetHandle();
        uint index;
        Win32.CoWaitForMultipleObjects(0, 0xFFFFFFFF, 1, &handle, &index);
    }
}
