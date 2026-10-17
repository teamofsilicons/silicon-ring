import AVFoundation
import Observation
import SwiftUI
import UIKit

/// The speaker control. On iPhone a tap moves the call between the earpiece (or a connected headset)
/// and the loudspeaker, and a long press picks a headset, car kit or the iPhone itself. iPad has no
/// earpiece, so the call is always on its speaker (or a headset) and the control only shows that.
struct AudioRouteButton: View {
    var onMoment = true
    var size: CGFloat = 64

    private var route: CallAudioRoute { .shared }

    var body: some View {
        let button = RoundCallButton(
            symbol: route.symbol, label: "Speaker", kind: route.isSpeaker ? .toggled : .normal,
            size: size, onMoment: onMoment
        ) {
            route.toggleSpeaker()
        }
        .disabled(!route.hasReceiver)
        .accessibilityValue(route.accessibilityValue)

        if route.hasReceiver && route.choices.count > 1 {
            button.contextMenu {
                ForEach(route.choices) { choice in
                    Button {
                        route.select(choice)
                    } label: {
                        if choice.isCurrent && !route.isSpeaker {
                            Label(choice.name, systemImage: "checkmark")
                        } else {
                            Text(choice.name)
                        }
                    }
                }
                Button {
                    route.setSpeaker(true)
                } label: {
                    if route.isSpeaker {
                        Label("Speaker", systemImage: "checkmark")
                    } else {
                        Text("Speaker")
                    }
                }
            }
        } else {
            button
        }
    }
}

/// Where this device plays the call. CallKit owns the audio session, so the speaker is an output
/// override, and iOS drops an override whenever the session is reconfigured (CallKit activating it,
/// LiveKit starting its voice-processing engine): the person's choice is kept here and put back.
@MainActor
@Observable
final class CallAudioRoute {
    static let shared = CallAudioRoute()

    enum Output: Equatable {
        case receiver, speaker, headphones, bluetooth, car, other
    }

    /// The iPhone itself, or a headset or car kit, each of which brings its own microphone.
    struct Choice: Identifiable, Equatable {
        let id: String
        let name: String
        let isCurrent: Bool
    }

    /// Only iPhone has an earpiece.
    let hasReceiver = UIDevice.current.userInterfaceIdiom == .phone
    private(set) var output: Output = .receiver
    private(set) var outputName = ""
    private(set) var choices: [Choice] = []
    /// What the person chose for this call.
    private var speakerOn = false
    @ObservationIgnored private var observer: NSObjectProtocol?

    private init() {
        observer = NotificationCenter.default.addObserver(
            forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main
        ) { notification in
            let raw = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
            let reason = raw.flatMap(AVAudioSession.RouteChangeReason.init(rawValue:))
            MainActor.assumeIsolated { CallAudioRoute.shared.routeChanged(reason) }
        }
        refresh()
    }

    var isSpeaker: Bool { output == .speaker }

    var symbol: String {
        switch output {
        case .receiver: "speaker.wave.2"
        case .speaker: "speaker.wave.3.fill"
        case .headphones, .bluetooth: "headphones"
        case .car: "car"
        case .other: "hifispeaker"
        }
    }

    var accessibilityValue: String {
        switch output {
        case .receiver: "Off"
        case .speaker: "On"
        default: outputName
        }
    }

    func toggleSpeaker() {
        setSpeaker(!isSpeaker)
    }

    func setSpeaker(_ on: Bool) {
        guard hasReceiver else { return }
        speakerOn = on
        apply()
    }

    /// Moves the call to a headset, a car kit or the iPhone (its earpiece).
    func select(_ choice: Choice) {
        let session = AVAudioSession.sharedInstance()
        guard let port = session.availableInputs?.first(where: { $0.uid == choice.id }) else { return }
        do {
            try session.setPreferredInput(port)
        } catch {
            DevLog.log("preferred input failed: \(error)")
        }
        setSpeaker(false)
    }

    /// A new call starts on the earpiece, or on a headset that is connected.
    func callStarting() {
        speakerOn = false
        try? AVAudioSession.sharedInstance().setPreferredInput(nil)
    }

    func sessionActivated() {
        apply()
    }

    func sessionDeactivated() {
        speakerOn = false
        refresh()
    }

    private func apply() {
        if hasReceiver {
            do {
                try AVAudioSession.sharedInstance().overrideOutputAudioPort(speakerOn ? .speaker : .none)
            } catch {
                DevLog.log("speaker override failed: \(error)")
            }
        }
        refresh()
    }

    private func routeChanged(_ reason: AVAudioSession.RouteChangeReason?) {
        refresh()
        guard hasReceiver else { return }
        switch reason {
        case .newDeviceAvailable:
            // A headset was connected: it takes the call, as in the Phone app.
            speakerOn = false
            if isSpeaker { apply() }
        case .override:
            // This app's override, or the speaker button on the system call screen.
            speakerOn = isSpeaker
        case .oldDeviceUnavailable:
            break
        default:
            // CallKit or LiveKit reconfigured the session, which drops the override: put it back.
            if speakerOn != isSpeaker, AVAudioSession.sharedInstance().category == .playAndRecord {
                apply()
            }
        }
    }

    private func refresh() {
        let session = AVAudioSession.sharedInstance()
        let port = session.currentRoute.outputs.first
        output = port.map { Self.output(for: $0.portType) } ?? (hasReceiver ? .receiver : .speaker)
        outputName = port?.portName ?? ""
        let currentInput = session.currentRoute.inputs.first?.uid
        let inputs = session.availableInputs ?? []
        choices = inputs.map { input in
            let name = input.portType == .builtInMic ? UIDevice.current.model : input.portName
            return Choice(id: input.uid, name: name, isCurrent: input.uid == currentInput)
        }
    }

    private static func output(for port: AVAudioSession.Port) -> Output {
        switch port {
        case .builtInReceiver: .receiver
        case .builtInSpeaker: .speaker
        case .headphones, .usbAudio: .headphones
        case .bluetoothHFP, .bluetoothA2DP, .bluetoothLE: .bluetooth
        case .carAudio: .car
        default: .other
        }
    }
}
