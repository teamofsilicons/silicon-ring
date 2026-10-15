using Windows.Media.Core;
using Windows.Media.Playback;
using Windows.Storage;

namespace Ring.Services;

public enum Sound
{
    Ringback,
    Ringtone,
    Connected,
    Ended,
}

/// <summary>
/// The Ring sounds from assets/sounds, played with a MediaPlayer. One loop (ringback or ringtone)
/// plays at a time; the short ticks play on their own player.
/// </summary>
public sealed class Sounds : IDisposable
{
    private readonly MediaPlayer _loop = new() { IsLoopingEnabled = true, AudioCategory = MediaPlayerAudioCategory.Communications };
    private readonly MediaPlayer _tick = new() { AudioCategory = MediaPlayerAudioCategory.Communications };
    private readonly Dictionary<Sound, StorageFile> _files = [];
    private Sound? _looping;
    private int _loopGeneration;

    public async Task PlayLoopAsync(Sound sound)
    {
        if (_looping == sound)
        {
            return;
        }
        var generation = ++_loopGeneration;
        _looping = sound;
        var file = await FileAsync(sound);
        // A Stop() or another loop may have arrived while the file was loading.
        if (file is null || generation != _loopGeneration)
        {
            return;
        }
        _loop.AudioCategory = sound == Sound.Ringtone ? MediaPlayerAudioCategory.Alerts : MediaPlayerAudioCategory.Communications;
        _loop.Source = MediaSource.CreateFromStorageFile(file);
        _loop.Play();
    }

    public void StopLoop()
    {
        _loopGeneration++;
        _looping = null;
        _loop.Pause();
        _loop.Source = null;
    }

    public bool IsLooping(Sound sound) => _looping == sound;

    public async Task PlayOnceAsync(Sound sound)
    {
        var file = await FileAsync(sound);
        if (file is null)
        {
            return;
        }
        _tick.Source = MediaSource.CreateFromStorageFile(file);
        _tick.Play();
    }

    public void Dispose()
    {
        _loop.Dispose();
        _tick.Dispose();
    }

    private async Task<StorageFile?> FileAsync(Sound sound)
    {
        if (_files.TryGetValue(sound, out var cached))
        {
            return cached;
        }
        var name = sound switch
        {
            Sound.Ringback => "ringback.wav",
            Sound.Ringtone => "ringtone.wav",
            Sound.Connected => "connected.wav",
            _ => "ended.wav",
        };
        try
        {
            var file = await StorageFile.GetFileFromPathAsync(AppPaths.Asset("Sounds", name));
            _files[sound] = file;
            return file;
        }
        catch (Exception ex) when (ex is FileNotFoundException or UnauthorizedAccessException or ArgumentException)
        {
            return null;
        }
    }
}
