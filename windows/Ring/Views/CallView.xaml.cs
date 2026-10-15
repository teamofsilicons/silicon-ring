using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Ring.Media;
using Ring.Services;
using Ring.ViewModels;

namespace Ring.Views;

/// <summary>Outgoing, in-call and voicemail screens for the call this device carries.</summary>
public sealed partial class CallView : UserControl
{
    public CallView()
    {
        InitializeComponent();
        // This view lives as long as the main window; the shell's notice bar is hidden under it.
        Services.Notice += OnNotice;
    }

    public ActiveCall? Call { get; private set; }

    private static AppServices Services => App.Services;

    public void Show(ActiveCall? call)
    {
        if (!ReferenceEquals(Call, call))
        {
            NoticeBar.IsOpen = false;
        }
        Call = call;
        Bindings.Update();
    }

    private void OnNotice(string message, bool isError)
    {
        if (Call is null)
        {
            return;
        }
        NoticeBar.Severity = isError ? InfoBarSeverity.Error : InfoBarSeverity.Informational;
        NoticeBar.Message = message;
        NoticeBar.IsOpen = true;
    }

    private void OnMuteClick(object sender, RoutedEventArgs e) => Services.Calls.ToggleMute();

    private async void OnHangUpClick(object sender, RoutedEventArgs e) => await Services.Calls.HangUpAsync();

    private async void OnAddPersonClick(object sender, RoutedEventArgs e)
    {
        var target = await DialerDialog.PickAsync(XamlRoot, invite: true);
        if (target is not null)
        {
            await Services.Calls.InviteAsync(target);
        }
    }

    private void OnDevicesFlyoutOpening(object? sender, object e)
    {
        DevicesFlyout.Items.Clear();
        if (!CallMedia.IsAvailable)
        {
            DevicesFlyout.Items.Add(new MenuFlyoutItem { Text = "Call audio is unavailable", IsEnabled = false });
            return;
        }
        AddGroup("MICROPHONE", "input", CallMedia.InputDevices(), Services.Preferences.InputDeviceId, Services.Calls.SetInputDevice);
        DevicesFlyout.Items.Add(new MenuFlyoutSeparator());
        AddGroup("SPEAKER", "output", CallMedia.OutputDevices(), Services.Preferences.OutputDeviceId, Services.Calls.SetOutputDevice);
    }

    private void AddGroup(string title, string group, IReadOnlyList<AudioDevice> devices, string? selected, Action<string?> choose)
    {
        DevicesFlyout.Items.Add(new MenuFlyoutItem { Text = title, IsEnabled = false });
        var choices = new List<(string? Id, string Name)> { (null, "System default") };
        choices.AddRange(devices.Select(d => ((string?)d.Id, d.Name)));
        foreach (var (id, name) in choices)
        {
            var item = new RadioMenuFlyoutItem { Text = name, GroupName = group, IsChecked = id == selected };
            item.Click += (_, _) => choose(id);
            DevicesFlyout.Items.Add(item);
        }
    }
}
