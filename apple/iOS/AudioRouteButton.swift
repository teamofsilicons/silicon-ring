import AVKit
import SwiftUI

/// Speaker and route picker: the system's AVRoutePickerView inside a round call control.
struct AudioRouteButton: View {
    var onMoment = true
    var size: CGFloat = 64

    var body: some View {
        VStack(spacing: 8) {
            ZStack {
                Circle().fill(onMoment ? Palette.momentSurface : Palette.beige)
                Circle().strokeBorder(onMoment ? Palette.momentMuted.opacity(0.35) : Palette.border, lineWidth: 0.5)
                Image(systemName: "speaker.wave.2")
                    .font(.system(size: size * 0.34, weight: .light))
                    .foregroundStyle(onMoment ? Palette.momentForeground : Palette.foreground)
                RoutePicker()
                    .frame(width: size, height: size)
                    .opacity(0.02)
            }
            .frame(width: size, height: size)
            Text("Speaker")
                .font(RingFont.pixel(11))
                .tracking(0.33)
                .textCase(.uppercase)
                .foregroundStyle(onMoment ? Palette.momentMuted : Palette.textMuted)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Audio output")
    }
}

private struct RoutePicker: UIViewRepresentable {
    func makeUIView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.prioritizesVideoDevices = false
        view.tintColor = .clear
        view.activeTintColor = .clear
        return view
    }

    func updateUIView(_ uiView: AVRoutePickerView, context: Context) {}
}
