import AppKit
import SwiftUI

/// Shown when `uv` cannot be found: how to install it, or where to point Shoal at it.
struct UVOnboardingView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openSettings) private var openSettings

    private let installCommand = "curl -LsSf https://astral.sh/uv/install.sh | sh"
    private let brewCommand = "brew install uv"

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Label("Install uv to run Shoal", systemImage: "shippingbox")
                .font(Theme.title)
                .foregroundStyle(Theme.text)
            Text("Shoal's simulation engine is a Python project that runs with uv, Astral's Python package manager. Install it with one of these commands in Terminal, then choose Try Again.")
                .font(Theme.body)
                .foregroundStyle(Theme.text)
                .fixedSize(horizontal: false, vertical: true)
            commandRow(installCommand)
            commandRow(brewCommand)
            Text("Already installed somewhere unusual? Set its path in Settings > Advanced.")
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
            HStack {
                Button("Open Settings…") { openSettings() }
                    .buttonStyle(.console)
                Spacer()
                Button("Quit Shoal") { NSApp.terminate(nil) }
                    .buttonStyle(.console)
                Button("Try Again") { model.engine.start() }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.consolePrimary)
            }
        }
        .padding(24)
        .frame(width: 520)
        .background(Theme.panel)
        .interactiveDismissDisabled()
    }

    private func commandRow(_ command: String) -> some View {
        HStack {
            Text(command)
                .font(Theme.mono(12))
                .foregroundStyle(Theme.text)
                .textSelection(.enabled)
                .lineLimit(1)
            Spacer()
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(command, forType: .string)
            } label: {
                Image(systemName: "doc.on.doc")
            }
            .buttonStyle(.plain)
            .foregroundStyle(Theme.muted)
            .help("Copy")
            .accessibilityLabel("Copy \(command)")
        }
        .padding(10)
        .elevatedSurface()
    }
}
