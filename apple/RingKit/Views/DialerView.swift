import SwiftUI

/// New call (or add person): an id field with a live lookup, and recent people below.
struct DialerView: View {
    var mode: DialerMode
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @State private var lookup: AccountLookup?
    @State private var lookupError: String?
    @State private var looking = false
    @State private var people: [RecentPerson] = []
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(mode == .call ? "New call" : "Add person")
                    .font(RingFont.display(26))
                    .foregroundStyle(Palette.ink)
                Spacer()
                Button("Close") { dismiss() }
                    .buttonStyle(.ringGhost)
                    .keyboardShortcut(.cancelAction)
            }
            .padding(.bottom, 16)

            HStack(spacing: 10) {
                Image(systemName: "at")
                    .font(.system(size: 15, weight: .light))
                    .foregroundStyle(Palette.textMuted)
                TextField("c:handle or si:handle", text: $text)
                    .font(RingFont.ui(16))
                    .textFieldStyle(.plain)
                    .autocorrectionDisabled()
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    .keyboardType(.asciiCapable)
                    .submitLabel(.go)
                    #endif
                    .focused($focused)
                    .onSubmit { if let lookup { call(lookup.account) } }
                if looking { ProgressView().controlSize(.small) }
            }
            .padding(.horizontal, 12)
            .frame(height: Metrics.controlHeight + 4)
            .background(RoundedRectangle(cornerRadius: Metrics.controlRadius, style: .continuous).fill(Palette.surfaceMuted))
            .overlay(RoundedRectangle(cornerRadius: Metrics.controlRadius, style: .continuous)
                .strokeBorder(focused ? Palette.accentBorder : Palette.borderStrong, lineWidth: focused ? 1 : 0.5))

            if let hint = hint {
                Text(hint)
                    .font(RingFont.ui(13))
                    .foregroundStyle(Palette.textMuted)
                    .padding(.top, 8)
            }

            if let lookup {
                resultCard(lookup)
                    .padding(.top, 16)
                    .transition(.opacity.combined(with: .scale(scale: 0.98)))
            } else if let lookupError {
                Text(lookupError)
                    .font(RingFont.ui(14))
                    .foregroundStyle(Palette.textSecondary)
                    .padding(.top, 16)
            }

            if !people.isEmpty {
                SectionLabel(text: "Recent people")
                    .padding(.top, 24)
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(people.enumerated()), id: \.element.id) { index, person in
                            if index > 0 { Hairline().padding(.leading, 56) }
                            Button {
                                call(person.account)
                            } label: {
                                HStack(spacing: 12) {
                                    AvatarView(account: person.account, size: 32)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(person.account.label).font(RingFont.ui(15, .medium)).foregroundStyle(Palette.foreground)
                                        Text(person.account.id).pixelID(10)
                                    }
                                    Spacer()
                                    KindBadge(account: person.account)
                                    Image(systemName: mode == .call ? "phone" : "person.badge.plus")
                                        .font(.system(size: 14, weight: .light))
                                        .foregroundStyle(Palette.accentText)
                                }
                                .padding(.horizontal, 14)
                                .padding(.vertical, 10)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .ringCard()
                }
            } else {
                Spacer()
            }
        }
        .padding(20)
        .frame(minWidth: 360, idealWidth: 440, minHeight: 420, idealHeight: 560)
        .background(Palette.background.ignoresSafeArea())
        .animation(.smooth(duration: 0.2), value: lookup?.account.uuid)
        .presentationDetents([.large])
        .onAppear { focused = true }
        .task(id: text) { await search() }
    }

    private var normalized: String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let stripped = trimmed.hasPrefix("@") ? String(trimmed.dropFirst()) : trimmed
        if stripped.hasPrefix("c:") || stripped.hasPrefix("si:") {
            let handle = stripped.split(separator: ":", maxSplits: 1).last.map(String.init) ?? ""
            return handle.isEmpty ? nil : stripped
        }
        if UUID(uuidString: stripped) != nil { return stripped }
        return nil
    }

    private var hint: String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, normalized == nil, !trimmed.contains(":") else { return nil }
        return "Add c: for a person or si: for a silicon, like c:\(trimmed) or si:\(trimmed)."
    }

    private func resultCard(_ lookup: AccountLookup) -> some View {
        HStack(spacing: 14) {
            AvatarView(account: lookup.account, size: 48)
            VStack(alignment: .leading, spacing: 4) {
                Text(lookup.account.label)
                    .font(RingFont.display(22))
                    .foregroundStyle(Palette.ink)
                    .lineLimit(1)
                Text(lookup.account.id).pixelID(11)
                HStack(spacing: 6) {
                    KindBadge(account: lookup.account, showCarbon: true)
                    Badge(text: lookup.onRing ? "On Ring" : "Not on Ring yet")
                }
                .padding(.top, 2)
            }
            Spacer(minLength: 8)
            Button {
                call(lookup.account)
            } label: {
                Label(mode == .call ? "Call" : "Add", systemImage: mode == .call ? "phone" : "person.badge.plus")
            }
            .buttonStyle(.ringPrimary)
        }
        .padding(14)
        .ringCard()
    }

    private func search() async {
        let query = text.trimmingCharacters(in: .whitespacesAndNewlines)
        try? await Task.sleep(for: .milliseconds(250))
        guard !Task.isCancelled else { return }
        if let recent = try? await model.api.recentPeople(query: query.hasPrefix("@") ? String(query.dropFirst()) : query, limit: 20) {
            guard !Task.isCancelled else { return }
            people = recent
        }
        guard let ref = normalized else {
            lookup = nil
            lookupError = nil
            return
        }
        looking = true
        defer { looking = false }
        do {
            let result = try await model.api.lookup(ref)
            guard !Task.isCancelled else { return }
            lookup = result
            lookupError = nil
        } catch let error as RingError {
            guard !Task.isCancelled else { return }
            lookup = nil
            switch error.code {
            case "account_not_found", "not_found": lookupError = "No one with the id \(ref)."
            case "cancelled": break
            default: lookupError = error.errorDescription
            }
        } catch {
            lookup = nil
        }
    }

    private func call(_ account: Account) {
        switch mode {
        case .call:
            dismiss()
            model.placeCall(to: account.uuid)
        case .invite:
            Task {
                if await model.invite(account.uuid) { dismiss() }
            }
        }
    }
}
