using System.Diagnostics;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;
using Ring.Api;
using Ring.Media;
using Ring.Services;
using Ring.ViewModels;

namespace Ring.Views;

public sealed partial class SettingsPage : Page
{
    // Programmatic updates must not echo back to the server as user changes.
    private bool _quiet = true;
    private RingSettings? _settings;

    public SettingsPage()
    {
        InitializeComponent();
    }

    private static AppServices Services => App.Services;

    protected override async void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        Services.ProfileChanged += ShowProfile;
        Services.DevicesChanged += OnDevicesChanged;

        _quiet = true;
        ShowProfile();
        ShowLocal();
        ShowAudioDevices();
        _quiet = false;

        await Task.WhenAll(LoadDevicesAsync(), LoadSettingsAsync());
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        Services.ProfileChanged -= ShowProfile;
        Services.DevicesChanged -= OnDevicesChanged;
    }

    private void OnDevicesChanged() => _ = LoadDevicesAsync();

    // ---- account and this device ----------------------------------------------------------------

    private void ShowProfile()
    {
        var wasQuiet = _quiet;
        _quiet = true;
        if (Services.Me is { } me)
        {
            AccountAvatar.Account = me;
            AccountName.Text = me.Label;
            AccountHandle.Text = Format.Pixel(me.Id);
        }
        // Leave the name alone while the user is editing it.
        if (DeviceNameBox.FocusState == FocusState.Unfocused)
        {
            DeviceNameBox.Text = Services.DeviceName ?? Environment.MachineName;
        }
        RingToggle.IsOn = Services.RingEnabled;
        _quiet = wasQuiet;
    }

    private void ShowLocal()
    {
        ThemeChoice.SelectedIndex = Services.Preferences.Theme switch
        {
            "light" => 1,
            "dark" => 2,
            _ => 0,
        };
        TrayToggle.IsOn = Services.Preferences.KeepRunningInTray;
        try
        {
            LoginToggle.IsOn = StartupRegistration.IsEnabled;
        }
        catch (Exception)
        {
            LoginToggle.IsEnabled = false;
        }
        var native = CallMedia.NativeVersion;
        VersionText.Text = native is null ? $"Ring {AppServices.AppVersion}" : $"Ring {AppServices.AppVersion} · media {native}";
        ServerText.Text = Format.Pixel(Services.Api.BaseUri.Host);
        SourceLink.NavigateUri = new Uri(AppServices.SourceUrl);
    }

    private void OnEditProfileClick(object sender, RoutedEventArgs e)
    {
        var accounts = Services.Meta?.AccountsUrl ?? "https://accounts.teamofsilicons.com";
        Process.Start(new ProcessStartInfo(accounts) { UseShellExecute = true });
    }

    private async void OnSaveDeviceNameClick(object sender, RoutedEventArgs e)
    {
        var name = DeviceNameBox.Text.Trim();
        if (name.Length == 0)
        {
            return;
        }
        try
        {
            await Services.RenameDeviceAsync(name);
            await LoadDevicesAsync();
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
        }
    }

    private async void OnRingToggled(object sender, RoutedEventArgs e)
    {
        if (_quiet)
        {
            return;
        }
        try
        {
            await Services.SetRingEnabledAsync(RingToggle.IsOn);
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
            _quiet = true;
            RingToggle.IsOn = Services.RingEnabled;
            _quiet = false;
        }
    }

    // ---- devices --------------------------------------------------------------------------------

    private async Task LoadDevicesAsync()
    {
        try
        {
            var devices = await Services.Api.GetDevicesAsync();
            DevicesList.ItemsSource = devices
                .OrderByDescending(d => d.Current)
                .ThenByDescending(d => d.LastSeenAt ?? d.CreatedAt)
                .Select(d => new DeviceItem(d))
                .ToList();
        }
        catch (ApiException)
        {
            // Shown again on the next visit.
        }
    }

    private async void OnRevokeClick(object sender, RoutedEventArgs e)
    {
        if (sender is not FrameworkElement { Tag: DeviceItem item })
        {
            return;
        }
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = $"Sign out {item.Name}?",
            Content = "That device stops ringing and must sign in again to use Ring.",
            PrimaryButtonText = "Sign out",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            Style = (Style)Application.Current.Resources["DefaultContentDialogStyle"],
        }.WithAppTheme();
        if (await dialog.ShowAsync() != ContentDialogResult.Primary)
        {
            return;
        }
        try
        {
            await Services.Api.RevokeDeviceAsync(item.Id);
            await LoadDevicesAsync();
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
        }
    }

    // ---- audio ----------------------------------------------------------------------------------

    private void ShowAudioDevices()
    {
        if (!CallMedia.IsAvailable)
        {
            InputCombo.IsEnabled = false;
            OutputCombo.IsEnabled = false;
            AudioNote.Text = "Call audio is unavailable: ring_ffi.dll is missing from the Ring folder.";
            AudioNote.Visibility = Visibility.Visible;
            return;
        }
        Fill(InputCombo, CallMedia.InputDevices(), Services.Preferences.InputDeviceId);
        Fill(OutputCombo, CallMedia.OutputDevices(), Services.Preferences.OutputDeviceId);
    }

    private static void Fill(ComboBox combo, IReadOnlyList<AudioDevice> devices, string? selectedId)
    {
        var choices = new List<AudioDeviceChoice> { new(null, "System default") };
        choices.AddRange(devices.Select(d => new AudioDeviceChoice(d.Id, d.IsDefault ? $"{d.Name} (default)" : d.Name)));
        combo.ItemsSource = choices;
        combo.SelectedItem = choices.FirstOrDefault(c => c.Id == selectedId) ?? choices[0];
    }

    private void OnInputChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_quiet && InputCombo.SelectedItem is AudioDeviceChoice choice)
        {
            Services.Calls.SetInputDevice(choice.Id);
        }
    }

    private void OnOutputChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_quiet && OutputCombo.SelectedItem is AudioDeviceChoice choice)
        {
            Services.Calls.SetOutputDevice(choice.Id);
        }
    }

    // ---- voicemail ------------------------------------------------------------------------------

    private async Task LoadSettingsAsync()
    {
        try
        {
            var settingsTask = Services.Api.GetSettingsAsync();
            var voicesTask = Services.Api.GetVoicesAsync();
            ShowSettings(await settingsTask, await voicesTask);
        }
        catch (ApiException ex) when (ex.Code != "network")
        {
            Services.ShowNotice(ex.UserMessage);
        }
        catch (ApiException)
        {
            // Offline.
        }
    }

    private void ShowSettings(RingSettings settings, VoiceList? voices)
    {
        _settings = settings;
        _quiet = true;
        VoicemailToggle.IsOn = settings.Voicemail.Enabled;
        BusyGreeting.Text = settings.Voicemail.Greetings.Busy ?? "";
        DeclinedGreeting.Text = settings.Voicemail.Greetings.Declined ?? "";
        MissedGreeting.Text = settings.Voicemail.Greetings.Missed ?? "";
        if (voices is not null)
        {
            var choices = voices.Items.Select(v => new VoiceChoice(v)).ToList();
            VoiceCombo.ItemsSource = choices;
            VoiceCombo.SelectedItem = choices.FirstOrDefault(c => c.Id == settings.Voice);
        }
        _quiet = false;
    }

    private async Task PatchAsync(System.Text.Json.Nodes.JsonObject patch)
    {
        try
        {
            var updated = await Services.Api.PatchSettingsAsync(patch);
            ShowSettings(updated, null);
        }
        catch (ApiException ex)
        {
            Services.ShowNotice(ex.UserMessage);
        }
    }

    private async void OnVoicemailToggled(object sender, RoutedEventArgs e)
    {
        if (!_quiet)
        {
            await PatchAsync(SettingsPatch.VoicemailEnabled(VoicemailToggle.IsOn));
        }
    }

    private async void OnVoiceChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_quiet && VoiceCombo.SelectedItem is VoiceChoice choice && choice.Id != _settings?.Voice)
        {
            await PatchAsync(SettingsPatch.Voice(choice.Id));
        }
    }

    private TextBox GreetingBox(string reason) => reason switch
    {
        "busy" => BusyGreeting,
        "declined" => DeclinedGreeting,
        _ => MissedGreeting,
    };

    private async void OnSaveGreetingClick(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: string reason })
        {
            var text = GreetingBox(reason).Text.Trim();
            await PatchAsync(SettingsPatch.Greeting(reason, text.Length == 0 ? null : text));
        }
    }

    private async void OnResetGreetingClick(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: string reason })
        {
            await PatchAsync(SettingsPatch.Greeting(reason, null));
        }
    }

    // ---- local preferences ----------------------------------------------------------------------

    private void OnThemeChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_quiet)
        {
            return;
        }
        Services.Preferences.Theme = ThemeChoice.SelectedIndex switch
        {
            1 => "light",
            2 => "dark",
            _ => "system",
        };
        Services.Preferences.Save();
        App.ApplyTheme();
    }

    private void OnLoginToggled(object sender, RoutedEventArgs e)
    {
        if (_quiet)
        {
            return;
        }
        try
        {
            StartupRegistration.SetEnabled(LoginToggle.IsOn);
            Services.Preferences.OpenAtLoginInitialized = true;
            Services.Preferences.Save();
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or System.Security.SecurityException or IOException)
        {
            Services.ShowNotice("Windows didn't allow Ring to change its startup setting.");
        }
    }

    private void OnTrayToggled(object sender, RoutedEventArgs e)
    {
        if (_quiet)
        {
            return;
        }
        Services.Preferences.KeepRunningInTray = TrayToggle.IsOn;
        Services.Preferences.Save();
        App.UpdateTray();
    }

    private async void OnSignOutClick(object sender, RoutedEventArgs e)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Sign out of Ring?",
            Content = "This PC stops ringing until you sign in again.",
            PrimaryButtonText = "Sign out",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Close,
            Style = (Style)Application.Current.Resources["DefaultContentDialogStyle"],
        }.WithAppTheme();
        if (await dialog.ShowAsync() == ContentDialogResult.Primary)
        {
            await Services.SignOutAsync();
        }
    }
}
