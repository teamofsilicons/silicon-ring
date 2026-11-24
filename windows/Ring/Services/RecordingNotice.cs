using Ring.ViewModels;

namespace Ring.Services;

/// <summary>
/// The notice that Ring records and transcribes calls, and which providers process the audio.
/// Each device asks once, right after sign-in and again before a call until it has agreed, and
/// remembers the answer in <see cref="Preferences"/>.
/// </summary>
public static class RecordingNotice
{
    /// <summary>Raise this when the notice's wording changes enough that people should agree again.</summary>
    public const int Version = 1;

    public static bool IsAccepted(Preferences preferences) => preferences.RecordingNoticeAccepted >= Version;

    public static void Accept(Preferences preferences)
    {
        preferences.RecordingNoticeAccepted = Version;
        preferences.Save();
    }

    /// <summary>Whether this sign-in has seen the current notice; the one after sign-in shows only until then.</summary>
    public static bool WasOffered(Preferences preferences) => preferences.RecordingNoticeOffered >= Version;

    public static void MarkOffered(Preferences preferences)
    {
        preferences.RecordingNoticeOffered = Version;
        preferences.Save();
    }

    /// <summary>Signing out: the next sign-in shows the notice again, unless this device has agreed.</summary>
    public static void ResetOffer(Preferences preferences)
    {
        preferences.RecordingNoticeOffered = 0;
        preferences.Save();
    }
}

public enum RecordingNoticeAction
{
    /// <summary>Right after sign-in, before any call: nothing waits.</summary>
    Welcome,
    /// <summary>Calling <see cref="RecordingNoticeRequest.To"/>.</summary>
    Place,
    /// <summary>Answering <see cref="RecordingNoticeRequest.Incoming"/> in the app.</summary>
    Answer,
    /// <summary>Moving the call <see cref="RecordingNoticeRequest.Ringid"/> here from another device.</summary>
    SwitchHere,
    /// <summary>
    /// A call this device already carries, connected before it agreed (answered from the toast, or
    /// rejoined after a restart). It goes on with the microphone muted
    /// (<see cref="ActiveCall.WaitsForNotice"/>), and the notice shows over its call screen:
    /// agreeing turns the microphone on, "Not now" ends the call.
    /// </summary>
    Answered,
}

/// <summary>Where the recording notice shows.</summary>
public enum RecordingNoticePlacement
{
    /// <summary>Over the whole window; nothing underneath responds until it is answered.</summary>
    Window,
    /// <summary>
    /// Over the people and transcript of the live call it is about, leaving end and the audio
    /// devices usable (mute stays on until the notice is answered).
    /// </summary>
    CallScreen,
}

/// <summary>The recording notice on screen, and the call action waiting for it.</summary>
public sealed class RecordingNoticeRequest
{
    private RecordingNoticeRequest(RecordingNoticeAction action)
    {
        Action = action;
    }

    public RecordingNoticeAction Action { get; }

    public string? To { get; private init; }

    public IncomingCall? Incoming { get; private init; }

    public string? Ringid { get; private init; }

    public bool IsAnswered => Action == RecordingNoticeAction.Answered;

    public static RecordingNoticeRequest Welcome() => new(RecordingNoticeAction.Welcome);

    public static RecordingNoticeRequest Place(string to) => new(RecordingNoticeAction.Place) { To = to };

    public static RecordingNoticeRequest Answer(IncomingCall incoming) => new(RecordingNoticeAction.Answer) { Incoming = incoming };

    public static RecordingNoticeRequest SwitchHere(string ringid) => new(RecordingNoticeAction.SwitchHere) { Ringid = ringid };

    public static RecordingNoticeRequest Answered() => new(RecordingNoticeAction.Answered);
}
