using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Media.Imaging;
using Ring.Api;
using Ring.ViewModels;

namespace Ring.Controls;

/// <summary>
/// A round avatar: the Accounts photo when there is one, otherwise the initial in Hedvig Letters
/// Serif on beige. While the person speaks, a soft accent ring pulses around it.
/// </summary>
public sealed partial class Avatar : UserControl
{
    public static readonly DependencyProperty AccountProperty =
        DependencyProperty.Register(nameof(Account), typeof(object), typeof(Avatar), new PropertyMetadata(null, OnAccountChanged));

    public static readonly DependencyProperty SizeProperty =
        DependencyProperty.Register(nameof(Size), typeof(double), typeof(Avatar), new PropertyMetadata(48.0, OnSizeChanged));

    public static readonly DependencyProperty IsSpeakingProperty =
        DependencyProperty.Register(nameof(IsSpeaking), typeof(bool), typeof(Avatar), new PropertyMetadata(false, OnSpeakingChanged));

    private Storyboard? _pulse;
    private string? _photoUrl;

    public Avatar()
    {
        InitializeComponent();
        ApplySize();
    }

    /// <summary>The <see cref="Ring.Api.Account"/> to show (typed as object for XAML).</summary>
    public object? Account
    {
        get => GetValue(AccountProperty);
        set => SetValue(AccountProperty, value);
    }

    public double Size
    {
        get => (double)GetValue(SizeProperty);
        set => SetValue(SizeProperty, value);
    }

    public bool IsSpeaking
    {
        get => (bool)GetValue(IsSpeakingProperty);
        set => SetValue(IsSpeakingProperty, value);
    }

    private static void OnAccountChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((Avatar)d).ApplyAccount();

    private static void OnSizeChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((Avatar)d).ApplySize();

    private static void OnSpeakingChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((Avatar)d).ApplySpeaking();

    private void ApplySize()
    {
        var size = Math.Max(16, Size);
        Root.Width = size;
        Root.Height = size;
        InitialText.FontSize = Math.Round(size * 0.46);
        SpeakingRing.Margin = new Thickness(-Math.Max(3, size * 0.1));
        if (_photoUrl is not null)
        {
            LoadPhoto(_photoUrl);
        }
    }

    private void ApplyAccount()
    {
        var account = Account as Account;
        InitialText.Text = Format.Initial(account);
        var url = account?.PfpUrl;
        if (string.IsNullOrWhiteSpace(url) || !Uri.TryCreate(url, UriKind.Absolute, out _))
        {
            _photoUrl = null;
            Photo.Visibility = Visibility.Collapsed;
            PhotoBrush.ImageSource = null;
            return;
        }
        if (url != _photoUrl)
        {
            _photoUrl = url;
            LoadPhoto(url);
        }
    }

    private void LoadPhoto(string url)
    {
        var bitmap = new BitmapImage
        {
            DecodePixelType = DecodePixelType.Logical,
            DecodePixelWidth = (int)Math.Ceiling(Math.Max(16, Size)),
        };
        bitmap.ImageOpened += (_, _) =>
        {
            if (_photoUrl == url)
            {
                Photo.Visibility = Visibility.Visible;
            }
        };
        bitmap.ImageFailed += (_, _) =>
        {
            if (_photoUrl == url)
            {
                Photo.Visibility = Visibility.Collapsed;
            }
        };
        bitmap.UriSource = new Uri(url);
        PhotoBrush.ImageSource = bitmap;
    }

    private void ApplySpeaking()
    {
        if (!IsSpeaking)
        {
            _pulse?.Stop();
            SpeakingRing.Opacity = 0;
            return;
        }
        if (!Motion.Enabled)
        {
            SpeakingRing.Opacity = 0.8;
            return;
        }
        if (_pulse is null)
        {
            _pulse = new Storyboard { RepeatBehavior = RepeatBehavior.Forever };
            var duration = new Duration(TimeSpan.FromMilliseconds(1100));
            var fade = new DoubleAnimation { From = 0.75, To = 0, Duration = duration };
            Storyboard.SetTarget(fade, SpeakingRing);
            Storyboard.SetTargetProperty(fade, "Opacity");
            var growX = new DoubleAnimation { From = 1, To = 1.22, Duration = duration };
            Storyboard.SetTarget(growX, SpeakingScale);
            Storyboard.SetTargetProperty(growX, "ScaleX");
            var growY = new DoubleAnimation { From = 1, To = 1.22, Duration = duration };
            Storyboard.SetTarget(growY, SpeakingScale);
            Storyboard.SetTargetProperty(growY, "ScaleY");
            _pulse.Children.Add(fade);
            _pulse.Children.Add(growX);
            _pulse.Children.Add(growY);
        }
        _pulse.Begin();
    }
}
