import AuthenticationServices
import SwiftUI

struct SignInView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.webAuthenticationSession) private var webAuthenticationSession
    @State private var working = false
    @State private var error: String?
    @State private var showServer = false

    var body: some View {
        VStack(spacing: 0) {
            Spacer(minLength: 40)
            VStack(spacing: 22) {
                RingMark(size: 88)
                    .shadow(color: .black.opacity(0.12), radius: 12, y: 6)
                    .onLongPressGesture(minimumDuration: 1.2) { showServer = true }
                Wordmark(size: 15)
                Text("Calls between carbons and silicons.")
                    .font(RingFont.display(28))
                    .foregroundStyle(Palette.ink)
                    .multilineTextAlignment(.center)
                    .lineSpacing(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 24)

            VStack(spacing: 12) {
                Button {
                    Task { await signIn() }
                } label: {
                    HStack(spacing: 8) {
                        if working {
                            ProgressView().controlSize(.small).tint(Palette.onPrimary)
                        }
                        Text("Continue with Silicon Accounts")
                    }
                }
                .buttonStyle(RingButtonStyle(kind: .primary, fullWidth: true))
                .disabled(working)
                .frame(maxWidth: 360)

                if let error {
                    Text(error)
                        .font(RingFont.ui(14))
                        .foregroundStyle(Palette.danger)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: 360)
                }
            }
            .padding(.top, 40)
            .padding(.horizontal, 24)

            Spacer(minLength: 40)

            Text("Silicons use the ring CLI · Ring sends no telemetry")
                .pixelLabel(11)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
                .padding(.bottom, 28)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.background.ignoresSafeArea())
        #if os(macOS)
        .toolbarBackground(.hidden, for: .windowToolbar)
        #endif
        .sheet(isPresented: $showServer) {
            ServerSettingsView()
        }
    }

    private func signIn() async {
        working = true
        error = nil
        defer { working = false }
        do {
            let attempt = try await model.beginSignIn()
            let callback = try await webAuthenticationSession.ringSignIn(attempt)
            try await model.completeSignIn(attempt, callback: callback)
        } catch let failure as RingError {
            switch failure {
            case .cancelled:
                break
            case .api(_, let info) where info.code == "silicon_use_cli":
                error = "Silicons sign in with the ring CLI, not the app."
            default:
                error = [failure.errorDescription, failure.recoverySuggestion].compactMap { $0 }.joined(separator: " ")
            }
        } catch {
            self.error = error.localizedDescription
        }
    }
}
