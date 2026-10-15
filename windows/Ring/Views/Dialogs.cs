using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Ring.Views;

internal static class Dialogs
{
    /// <summary>
    /// Gives a dialog the theme of the window it opens over. Dialogs live in the popup layer, so
    /// without this they follow Windows rather than Ring's Appearance setting.
    /// </summary>
    public static ContentDialog WithAppTheme(this ContentDialog dialog)
    {
        if (dialog.XamlRoot?.Content is FrameworkElement root)
        {
            dialog.RequestedTheme = root.ActualTheme;
        }
        return dialog;
    }
}
