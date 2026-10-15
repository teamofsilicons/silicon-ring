import AVFoundation
import SwiftUI

/// Plays a voicemail or call recording from a (presigned) URL.
@MainActor
@Observable
final class AudioPlayback {
    private(set) var isPlaying = false
    private(set) var current: Double = 0
    private(set) var duration: Double
    private(set) var failed = false

    @ObservationIgnored private var player: AVPlayer?
    @ObservationIgnored private var timeObserver: Any?
    @ObservationIgnored private var endObserver: NSObjectProtocol?
    @ObservationIgnored private var url: URL?

    init(durationMS: Int? = nil) {
        duration = Double(durationMS ?? 0) / 1000
    }

    func load(_ url: URL) {
        guard url != self.url else { return }
        stop()
        self.url = url
        let item = AVPlayerItem(url: url)
        let player = AVPlayer(playerItem: item)
        self.player = player
        timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 0.25, preferredTimescale: 600), queue: .main) { [weak self] time in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.current = time.seconds.isFinite ? time.seconds : 0
                if let total = self.player?.currentItem?.duration.seconds, total.isFinite, total > 0 { self.duration = total }
                self.failed = self.player?.currentItem?.status == .failed
            }
        }
        endObserver = NotificationCenter.default.addObserver(forName: AVPlayerItem.didPlayToEndTimeNotification, object: item, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.isPlaying = false
                self?.player?.seek(to: .zero)
            }
        }
    }

    func toggle() {
        guard let player else { return }
        if isPlaying {
            player.pause()
            isPlaying = false
        } else {
            #if os(iOS)
            // Outside a call, play through the speaker: the session keeps CallKit's playAndRecord
            // (receiver) category after a call ends.
            if AppModel.shared.activeCall?.isLive != true {
                let session = AVAudioSession.sharedInstance()
                try? session.setCategory(.playback, mode: .spokenAudio)
                try? session.setActive(true)
            }
            #endif
            player.play()
            isPlaying = true
        }
    }

    func seek(to seconds: Double) {
        current = seconds
        player?.seek(to: CMTime(seconds: seconds, preferredTimescale: 600), toleranceBefore: .zero, toleranceAfter: .zero)
    }

    func stop() {
        player?.pause()
        if let timeObserver { player?.removeTimeObserver(timeObserver) }
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        timeObserver = nil
        endObserver = nil
        player = nil
        url = nil
        isPlaying = false
        current = 0
    }
}

struct AudioPlayerView: View {
    var url: URL
    var durationMS: Int?
    @State private var playback: AudioPlayback

    init(url: URL, durationMS: Int?) {
        self.url = url
        self.durationMS = durationMS
        _playback = State(initialValue: AudioPlayback(durationMS: durationMS))
    }

    var body: some View {
        HStack(spacing: 14) {
            Button {
                playback.toggle()
            } label: {
                Image(systemName: playback.isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 16, weight: .regular))
                    .foregroundStyle(Palette.onPrimary)
                    .frame(width: 44, height: 44)
                    .background(Circle().fill(Palette.accent))
            }
            .buttonStyle(PressScaleStyle())
            .accessibilityLabel(playback.isPlaying ? "Pause" : "Play")

            VStack(spacing: 6) {
                Slider(value: Binding(get: { playback.current }, set: { playback.seek(to: $0) }),
                       in: 0...max(playback.duration, 0.1))
                    .tint(Palette.accent)
                HStack {
                    Text(Format.duration(seconds: Int(playback.current))).pixelLabel(10)
                    Spacer()
                    Text(Format.duration(seconds: Int(playback.duration))).pixelLabel(10)
                }
            }
        }
        .onAppear { playback.load(url) }
        .onDisappear { playback.stop() }
        .onChange(of: url) { _, new in playback.load(new) }
    }
}
