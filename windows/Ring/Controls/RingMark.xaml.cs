using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace Ring.Controls;

/// <summary>
/// The Ring mark: concentric rings around a dot. With <see cref="ShowTile"/> it is the app icon
/// (cream on an ink tile); otherwise the rings take <see cref="Mark"/>, so they follow the theme.
/// </summary>
public sealed partial class RingMark : UserControl
{
    private static readonly SolidColorBrush Cream = new(ColorHelper.FromArgb(0xFF, 0xED, 0xE8, 0xE0));

    public static readonly DependencyProperty ShowTileProperty =
        DependencyProperty.Register(nameof(ShowTile), typeof(bool), typeof(RingMark), new PropertyMetadata(false, OnChanged));

    public static readonly DependencyProperty MarkProperty =
        DependencyProperty.Register(nameof(Mark), typeof(Brush), typeof(RingMark), new PropertyMetadata(null, OnChanged));

    public RingMark()
    {
        InitializeComponent();
        Apply();
    }

    public bool ShowTile
    {
        get => (bool)GetValue(ShowTileProperty);
        set => SetValue(ShowTileProperty, value);
    }

    public Brush? Mark
    {
        get => (Brush?)GetValue(MarkProperty);
        set => SetValue(MarkProperty, value);
    }

    private static void OnChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((RingMark)d).Apply();

    private void Apply()
    {
        Tile.Visibility = ShowTile ? Visibility.Visible : Visibility.Collapsed;
        var brush = ShowTile ? Cream : Mark ?? Foreground ?? Cream;
        Dot.Fill = brush;
        InnerRing.Stroke = brush;
        MiddleRing.Stroke = brush;
        OuterRing.Stroke = brush;
    }
}
