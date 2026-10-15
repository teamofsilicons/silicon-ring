import LiveKit
import ServiceManagement
import SwiftUI

/// The Mac's output picker in the call controls.
struct AudioRouteButton: View {
    var onMoment = true
    var size: CGFloat = 64
    @Environment(AppModel.self) private var model
    @State private var devices: [AudioDevice] = []
    @State private var selected = ""

    var body: some View {
        Menu {
            ForEach(devices, id: \.deviceId) { device in
                Button {
                    model.media.selectOutput(device)
                    selected = device.deviceId
                } label: {
                    if device.deviceId == selected {
                        Label(device.name, systemImage: "checkmark")
                    } else {
                        Text(device.name)
                    }
                }
            }
        } label: {
            VStack(spacing: 8) {
                Image(systemName: "speaker.wave.2")
                    .font(.system(size: size * 0.34, weight: .light))
                    .foregroundStyle(onMoment ? Palette.momentForeground : Palette.foreground)
                    .frame(width: size, height: size)
                    .background(Circle().fill(onMoment ? Palette.momentSurface : Palette.beige))
                    .overlay(Circle().strokeBorder(onMoment ? Palette.momentMuted.opacity(0.35) : Palette.border, lineWidth: 0.5))
                Text("Output")
                    .font(RingFont.pixel(11))
                    .tracking(0.33)
                    .textCase(.uppercase)
                    .foregroundStyle(onMoment ? Palette.momentMuted : Palette.textMuted)
            }
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .onAppear {
            devices = model.media.outputDevices
            selected = model.media.outputDeviceID
        }
        .accessibilityLabel("Audio output")
    }
}

/// Settings that exist only on the desktop.
struct DesktopSettingsSection: View {
    @State private var openAtLogin = DesktopPreferences.openAtLogin
    @State private var needsApproval = DesktopPreferences.loginNeedsApproval
    @State private var keepInMenuBar = DesktopPreferences.keepInMenuBar
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionLabel(text: "Desktop")
            VStack(alignment: .leading, spacing: 0) {
                SettingToggle(
                    title: "Open at login",
                    detail: "Ring can only ring this Mac while it is running.",
                    isOn: Binding(get: { openAtLogin }, set: setOpenAtLogin))
                if needsApproval {
                    Button("Allow Ring in Login Items…") { SMAppService.openSystemSettingsLoginItems() }
                        .buttonStyle(.ringGhost)
                        .padding(.horizontal, 8)
                        .padding(.bottom, 8)
                }
                Hairline().padding(.leading, 14)
                SettingToggle(
                    title: "Keep running in the menu bar",
                    detail: "Closing the window keeps Ring listening for calls.",
                    isOn: Binding(get: { keepInMenuBar }, set: { value in
                        keepInMenuBar = value
                        DesktopPreferences.keepInMenuBar = value
                    }))
                if let error {
                    Text(error).font(RingFont.ui(13)).foregroundStyle(Palette.danger).padding(14)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .ringCard()
        }
    }

    private func setOpenAtLogin(_ enabled: Bool) {
        do {
            try DesktopPreferences.setOpenAtLogin(enabled)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        openAtLogin = DesktopPreferences.openAtLogin
        needsApproval = DesktopPreferences.loginNeedsApproval
    }
}
