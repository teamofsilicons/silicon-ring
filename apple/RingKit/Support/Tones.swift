import AVFoundation
import Foundation
#if os(iOS)
import AudioToolbox
#endif

/// Ring's local sounds: the caller's ringback, the in-app ringtone and the short ticks.
@MainActor
final class Tones {
    static let shared = Tones()

    enum Tick: String {
        case connected, ended
    }

    private var ringback: AVAudioPlayer?
    private var ringtone: AVAudioPlayer?
    private var tick: AVAudioPlayer?
    #if os(iOS)
    private var vibration: Task<Void, Never>?
    #endif

    var isRingbackPlaying: Bool { ringback?.isPlaying == true }

    /// "Tring tring, pause" on a 3 s cycle until the call is answered or goes to voicemail.
    /// On iOS this must run inside the CallKit-activated audio session.
    func startRingback() {
        guard ringback?.isPlaying != true else { return }
        ringback = loop("ringback")
        ringback?.play()
    }

    func stopRingback() {
        ringback?.stop()
        ringback = nil
    }

    /// The in-app ringtone, used when the app shows its own incoming-call UI.
    func startRingtone() {
        guard ringtone?.isPlaying != true else { return }
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        // Respect the ring/silent switch like a ringer does. Callers never ring over a live call;
        // the category alone can't tell, since it stays playAndRecord after every CallKit call.
        guard AppModel.shared.activeCall?.isLive != true else { return }
        try? session.setCategory(.soloAmbient)
        try? session.setActive(true)
        vibration = Task {
            while !Task.isCancelled {
                AudioServicesPlayAlertSound(SystemSoundID(kSystemSoundID_Vibrate))
                try? await Task.sleep(for: .seconds(3))
            }
        }
        #endif
        ringtone = loop("ringtone")
        ringtone?.play()
    }

    func stopRingtone() {
        let wasPlaying = ringtone != nil
        ringtone?.stop()
        ringtone = nil
        #if os(iOS)
        vibration?.cancel()
        vibration = nil
        if wasPlaying, AVAudioSession.sharedInstance().category == .soloAmbient {
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        }
        #else
        _ = wasPlaying
        #endif
    }

    func play(_ tick: Tick) {
        guard let url = Bundle.main.url(forResource: tick.rawValue, withExtension: "wav"),
              let player = try? AVAudioPlayer(contentsOf: url) else { return }
        player.volume = 0.6
        player.play()
        self.tick = player
    }

    func stopAll() {
        stopRingback()
        stopRingtone()
    }

    private func loop(_ name: String) -> AVAudioPlayer? {
        guard let url = Bundle.main.url(forResource: name, withExtension: "wav"),
              let player = try? AVAudioPlayer(contentsOf: url) else { return nil }
        player.numberOfLoops = -1
        player.prepareToPlay()
        return player
    }
}
