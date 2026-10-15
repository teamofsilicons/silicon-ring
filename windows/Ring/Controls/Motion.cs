using Windows.UI.ViewManagement;

namespace Ring.Controls;

/// <summary>Honours "Animation effects" in Windows settings: when off, Ring uses fades only.</summary>
internal static class Motion
{
    private static readonly UISettings Settings = new();

    public static bool Enabled
    {
        get
        {
            try
            {
                return Settings.AnimationsEnabled;
            }
            catch (Exception)
            {
                return true;
            }
        }
    }
}
