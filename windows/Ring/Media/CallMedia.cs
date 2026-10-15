using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;
using Ring.Api;

namespace Ring.Media;

public sealed record AudioDevice(string Id, string Name, bool IsDefault);

/// <summary>One event from ring_ffi (see the event list in ring_ffi.h).</summary>
public sealed class MediaEvent
{
    public string Type { get; init; } = "";
    public string? State { get; init; }
    public string? Reason { get; init; }
    public string? Identity { get; init; }
    public List<string> Identities { get; init; } = [];
    public double Level { get; init; }
    public bool Muted { get; init; }
}

public static class MediaEventTypes
{
    public const string State = "state";
    public const string ParticipantJoined = "participant_joined";
    public const string ParticipantLeft = "participant_left";
    public const string ActiveSpeakers = "active_speakers";
    public const string LocalLevel = "local_level";
    public const string RemoteMuted = "remote_muted";
}

/// <summary>
/// Call audio for one LiveKit room through ring_ffi.dll: one microphone track published, every
/// remote track played, with WebRTC echo cancellation. Events are raised on a background thread.
/// </summary>
public sealed unsafe class CallMedia
{
    private readonly object _gate = new();
    private nint _handle;
    private GCHandle _self;

    private readonly Action<CallMedia, MediaEvent> _onEvent;

    private CallMedia(MediaGrant grant, Action<CallMedia, MediaEvent> onEvent)
    {
        Grant = grant;
        _onEvent = onEvent;
    }

    public MediaGrant Grant { get; }

    /// <summary>The local participant's LiveKit identity.</summary>
    public string LocalIdentity => Grant.Identity;

    /// <summary>Whether ring_ffi.dll can be loaded at all.</summary>
    public static bool IsAvailable
    {
        get
        {
            try
            {
                return RingFfi.Version() != 0;
            }
            catch (Exception ex) when (ex is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
            {
                return false;
            }
        }
    }

    public static string? NativeVersion
    {
        get
        {
            try
            {
                var ptr = RingFfi.Version();
                return ptr == 0 ? null : Marshal.PtrToStringUTF8(ptr);
            }
            catch (Exception ex) when (ex is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
            {
                return null;
            }
        }
    }

    /// <summary>
    /// Starts joining the room and returns at once; progress arrives as <c>state</c> events on
    /// <paramref name="onEvent"/>, on a background thread. The handler is in place before the
    /// native side starts, so an early failure is never missed.
    /// Throws <see cref="MediaUnavailableException"/> when the native library is missing or refuses.
    /// </summary>
    public static CallMedia Connect(MediaGrant grant, string? inputDeviceId, string? outputDeviceId, bool muted, Action<CallMedia, MediaEvent> onEvent)
    {
        var media = new CallMedia(grant, onEvent);
        var options = new JsonObject
        {
            ["input_device"] = inputDeviceId,
            ["output_device"] = outputDeviceId,
            ["muted"] = muted,
            ["room"] = grant.Room,
            ["identity"] = grant.Identity,
        };
        media._self = GCHandle.Alloc(media);
        try
        {
            delegate* unmanaged[Cdecl]<nint, byte*, void> callback = &OnNativeEvent;
            var handle = RingFfi.Connect(grant.Url, grant.Token, options.ToJsonString(), (nint)callback, GCHandle.ToIntPtr(media._self));
            if (handle == 0)
            {
                throw new MediaUnavailableException(RingFfi.LastErrorMessage());
            }
            media._handle = handle;
            return media;
        }
        catch (Exception ex) when (ex is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        {
            media._self.Free();
            throw new MediaUnavailableException("Call audio is unavailable because ring_ffi.dll could not be loaded.", ex);
        }
        catch
        {
            if (media._self.IsAllocated)
            {
                media._self.Free();
            }
            throw;
        }
    }

    public bool IsMuted
    {
        get
        {
            lock (_gate)
            {
                return _handle != 0 && RingFfi.IsMuted(_handle) == 1;
            }
        }
    }

    public void SetMuted(bool muted)
    {
        lock (_gate)
        {
            if (_handle != 0)
            {
                RingFfi.SetMuted(_handle, muted ? 1 : 0);
            }
        }
    }

    /// <summary>Switches the microphone; null selects the system default.</summary>
    public bool SetInputDevice(string? id)
    {
        lock (_gate)
        {
            return _handle != 0 && RingFfi.SetInputDevice(_handle, id) == RingFfi.Ok;
        }
    }

    /// <summary>Switches the speaker or headset; null selects the system default.</summary>
    public bool SetOutputDevice(string? id)
    {
        lock (_gate)
        {
            return _handle != 0 && RingFfi.SetOutputDevice(_handle, id) == RingFfi.Ok;
        }
    }

    /// <summary>Leaves the room and releases the audio devices. Safe to call more than once.</summary>
    public Task DisconnectAsync()
    {
        nint handle;
        lock (_gate)
        {
            handle = _handle;
            _handle = 0;
        }
        if (handle == 0)
        {
            return Task.CompletedTask;
        }
        return Task.Run(() =>
        {
            // ring_media_disconnect blocks until LiveKit has left; no callback runs afterwards,
            // so the GC handle can go.
            RingFfi.Disconnect(handle);
            if (_self.IsAllocated)
            {
                _self.Free();
            }
        });
    }

    public static IReadOnlyList<AudioDevice> InputDevices() => ParseDevices(() => RingFfi.InputDevicesJson());

    public static IReadOnlyList<AudioDevice> OutputDevices() => ParseDevices(() => RingFfi.OutputDevicesJson());

    private static IReadOnlyList<AudioDevice> ParseDevices(Func<nint> query)
    {
        try
        {
            var json = RingFfi.TakeString(query());
            if (string.IsNullOrEmpty(json))
            {
                return [];
            }
            using var doc = JsonDocument.Parse(json);
            var devices = new List<AudioDevice>();
            foreach (var item in doc.RootElement.EnumerateArray())
            {
                var id = item.TryGetProperty("id", out var idValue) ? idValue.GetString() : null;
                var name = item.TryGetProperty("name", out var nameValue) ? nameValue.GetString() : null;
                var isDefault = item.TryGetProperty("is_default", out var def) && def.ValueKind == JsonValueKind.True;
                if (!string.IsNullOrEmpty(id))
                {
                    devices.Add(new AudioDevice(id, string.IsNullOrWhiteSpace(name) ? id : name, isDefault));
                }
            }
            return devices;
        }
        catch (Exception ex) when (ex is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException or JsonException or InvalidOperationException)
        {
            return [];
        }
    }

    private void Dispatch(string json)
    {
        MediaEvent? parsed;
        try
        {
            parsed = JsonSerializer.Deserialize<MediaEvent>(json, RingJson.Options);
        }
        catch (JsonException)
        {
            return;
        }
        if (parsed is not null)
        {
            _onEvent(this, parsed);
        }
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnNativeEvent(nint context, byte* eventJson)
    {
        // Exceptions must never unwind into native code.
        try
        {
            if (context == 0 || eventJson is null)
            {
                return;
            }
            if (GCHandle.FromIntPtr(context).Target is CallMedia media)
            {
                media.Dispatch(Marshal.PtrToStringUTF8((nint)eventJson) ?? "");
            }
        }
        catch (Exception)
        {
            // Dropped: a malformed event is not worth a crash.
        }
    }
}

public sealed class MediaUnavailableException : Exception
{
    public MediaUnavailableException(string message, Exception? inner = null) : base(message, inner)
    {
    }
}
