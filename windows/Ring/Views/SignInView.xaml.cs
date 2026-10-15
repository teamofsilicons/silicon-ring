using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Ring.Api;
using Ring.Services;

namespace Ring.Views;

/// <summary>Sign in: hosted Silicon Accounts in the browser, back through a loopback redirect.</summary>
public sealed partial class SignInView : UserControl
{
    private CancellationTokenSource? _signIn;

    public SignInView()
    {
        InitializeComponent();
        Loaded += (_, _) => UpdateServerLabel();
        // This view lives as long as the main window; notices while signed out land here.
        Services.Notice += OnNotice;
        Services.SessionChanged += () => NoticeBar.IsOpen = NoticeBar.IsOpen && !Services.IsSignedIn;
    }

    private static AppServices Services => App.Services;

    private void OnNotice(string message, bool isError)
    {
        if (Services.IsSignedIn)
        {
            return;
        }
        NoticeBar.Severity = isError ? InfoBarSeverity.Error : InfoBarSeverity.Informational;
        NoticeBar.Message = message;
        NoticeBar.IsOpen = true;
    }

    private void UpdateServerLabel()
    {
        var url = Services.Api.BaseUri.ToString().TrimEnd('/');
        ServerButton.Content = url == AppServices.ProductionApiUrl ? "Server: production" : $"Server: {url}";
    }

    private async void OnSignInClick(object sender, RoutedEventArgs e)
    {
        ErrorText.Visibility = Visibility.Collapsed;
        SignInButton.IsEnabled = false;
        WaitingPanel.Visibility = Visibility.Visible;
        _signIn = new CancellationTokenSource(TimeSpan.FromMinutes(10));
        try
        {
            await Services.SignInAsync(_signIn.Token);
        }
        catch (OperationCanceledException)
        {
            // Cancelled here or in the browser.
        }
        catch (ApiException ex)
        {
            ShowError(ex.Code switch
            {
                "silicon_use_cli" => "Silicons use the ring CLI. Sign in with a carbon (c:) account here.",
                "network" => "Ring can't reach the server. Check your connection and try again.",
                _ => ex.UserMessage,
            });
        }
        catch (Exception ex) when (ex is System.Net.HttpListenerException or System.ComponentModel.Win32Exception or IOException)
        {
            ShowError($"Sign-in could not start: {ex.Message}");
        }
        finally
        {
            _signIn?.Dispose();
            _signIn = null;
            SignInButton.IsEnabled = true;
            WaitingPanel.Visibility = Visibility.Collapsed;
        }
    }

    private void OnCancelClick(object sender, RoutedEventArgs e) => _signIn?.Cancel();

    private void ShowError(string message)
    {
        ErrorText.Text = message;
        ErrorText.Visibility = Visibility.Visible;
    }

    private async void OnServerClick(object sender, RoutedEventArgs e)
    {
        var box = new TextBox
        {
            Text = Services.Api.BaseUri.ToString().TrimEnd('/'),
            PlaceholderText = AppServices.ProductionApiUrl,
            MinWidth = 360,
        };
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "Ring server",
            Content = box,
            PrimaryButtonText = "Use server",
            SecondaryButtonText = "Production",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
            Style = (Style)Application.Current.Resources["DefaultContentDialogStyle"],
        }.WithAppTheme();
        var result = await dialog.ShowAsync();
        if (result == ContentDialogResult.Primary)
        {
            var text = box.Text.Trim();
            if (Uri.TryCreate(text, UriKind.Absolute, out var uri) && (uri.Scheme == Uri.UriSchemeHttps || uri.Scheme == Uri.UriSchemeHttp))
            {
                Services.UseServer(text);
            }
            else
            {
                ShowError("Enter a server address such as https://backend.ring.teamofsilicons.com.");
            }
        }
        else if (result == ContentDialogResult.Secondary)
        {
            Services.UseServer(null);
        }
        UpdateServerLabel();
    }
}
