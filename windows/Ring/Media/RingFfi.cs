using System.Runtime.InteropServices;

namespace Ring.Media;

/// <summary>
/// P/Invoke surface of ring_ffi.dll (crates/ring-ffi). The C contract is windows/native/ring_ffi.h.
/// </summary>
internal static unsafe partial class RingFfi
{
    private const string Library = "ring_ffi";

    public const int Ok = 0;
    public const int ErrInvalidArgument = -1;
    public const int ErrMedia = -2;
    public const int ErrInternal = -3;

    [LibraryImport(Library, EntryPoint = "ring_ffi_version")]
    public static partial nint Version();

    [LibraryImport(Library, EntryPoint = "ring_ffi_last_error")]
    public static partial nint LastError();

    [LibraryImport(Library, EntryPoint = "ring_string_free")]
    public static partial void StringFree(nint value);

    /// <param name="callback">A <c>delegate* unmanaged[Cdecl]&lt;nint, byte*, void&gt;</c>.</param>
    [LibraryImport(Library, EntryPoint = "ring_media_connect", StringMarshalling = StringMarshalling.Utf8)]
    public static partial nint Connect(string url, string token, string? optionsJson, nint callback, nint context);

    [LibraryImport(Library, EntryPoint = "ring_media_set_muted")]
    public static partial int SetMuted(nint media, int muted);

    [LibraryImport(Library, EntryPoint = "ring_media_is_muted")]
    public static partial int IsMuted(nint media);

    [LibraryImport(Library, EntryPoint = "ring_media_set_input_device", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int SetInputDevice(nint media, string? deviceId);

    [LibraryImport(Library, EntryPoint = "ring_media_set_output_device", StringMarshalling = StringMarshalling.Utf8)]
    public static partial int SetOutputDevice(nint media, string? deviceId);

    [LibraryImport(Library, EntryPoint = "ring_media_disconnect")]
    public static partial void Disconnect(nint media);

    [LibraryImport(Library, EntryPoint = "ring_media_input_devices_json")]
    public static partial nint InputDevicesJson();

    [LibraryImport(Library, EntryPoint = "ring_media_output_devices_json")]
    public static partial nint OutputDevicesJson();

    /// <summary>Copies and frees a string the library returned.</summary>
    public static string? TakeString(nint value)
    {
        if (value == 0)
        {
            return null;
        }
        try
        {
            return Marshal.PtrToStringUTF8(value);
        }
        finally
        {
            StringFree(value);
        }
    }

    public static string LastErrorMessage() => TakeString(LastError()) ?? "unknown media error";
}
