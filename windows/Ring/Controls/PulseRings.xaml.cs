using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Shapes;

namespace Ring.Controls;

public sealed partial class PulseRings : UserControl
{
    public static readonly DependencyProperty IsActiveProperty =
        DependencyProperty.Register(nameof(IsActive), typeof(bool), typeof(PulseRings), new PropertyMetadata(false, OnActiveChanged));

    private Storyboard? _storyboard;

    public PulseRings()
    {
        InitializeComponent();
        Unloaded += (_, _) => _storyboard?.Stop();
        Loaded += (_, _) => Apply();
    }

    public bool IsActive
    {
        get => (bool)GetValue(IsActiveProperty);
        set => SetValue(IsActiveProperty, value);
    }

    private static void OnActiveChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((PulseRings)d).Apply();

    private void Apply()
    {
        if (!IsActive || !Motion.Enabled)
        {
            _storyboard?.Stop();
            return;
        }
        _storyboard ??= Build();
        _storyboard.Begin();
    }

    private Storyboard Build()
    {
        var storyboard = new Storyboard();
        var cycle = TimeSpan.FromSeconds(3);
        var rings = new[] { Ring1, Ring2, Ring3 };
        for (var i = 0; i < rings.Length; i++)
        {
            Add(storyboard, rings[i], cycle, TimeSpan.FromSeconds(i));
        }
        return storyboard;
    }

    private static void Add(Storyboard storyboard, Ellipse ring, TimeSpan cycle, TimeSpan offset)
    {
        var duration = new Duration(cycle);
        var fade = new DoubleAnimation { From = 0.5, To = 0, Duration = duration, BeginTime = offset, RepeatBehavior = RepeatBehavior.Forever };
        Storyboard.SetTarget(fade, ring);
        Storyboard.SetTargetProperty(fade, "Opacity");
        storyboard.Children.Add(fade);
        foreach (var axis in new[] { "ScaleX", "ScaleY" })
        {
            var grow = new DoubleAnimation
            {
                From = 1,
                To = 1.9,
                Duration = duration,
                BeginTime = offset,
                RepeatBehavior = RepeatBehavior.Forever,
                EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
            };
            Storyboard.SetTarget(grow, ring);
            Storyboard.SetTargetProperty(grow, $"(UIElement.RenderTransform).(ScaleTransform.{axis})");
            storyboard.Children.Add(grow);
        }
    }
}
