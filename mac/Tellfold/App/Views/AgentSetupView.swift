import AppKit
import SwiftUI

/// The first-run step (also opened from Settings > Agents and the New simulation sheet): lists the AI tools found on
/// this Mac, preselects the suggested one and makes the choice the default for the crowd and the planner. With nothing
/// installed it shows where to get one.
struct AgentSetupView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    @State private var selection: String?
    @State private var isDetecting = false
    @State private var isSaving = false

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: Theme.space4) {
                header
                if isDetecting && model.detectedRuntimes.isEmpty {
                    detectingState
                } else if nothingFound {
                    installOptions
                } else {
                    choices
                }
            }
            .padding(Theme.space5)
            Seam()
            footer
        }
        .frame(width: 580)
        .background(Theme.ground)
        .task { await detect() }
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .top, spacing: Theme.space4) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 52, height: 52)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: Theme.space1) {
                Text(nothingFound ? "Tellfold needs an AI tool" : "Choose your AI tool")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Text("Tellfold has no AI of its own. It plays the crowd with an AI tool you already have on this Mac, "
                     + "using your own plan or a model that runs locally.")
                    .font(Theme.body)
                    .foregroundStyle(Theme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var detectingState: some View {
        HStack(spacing: Theme.space2) {
            ProgressView().controlSize(.small)
            Text("Looking for AI tools on this Mac…").font(Theme.body).foregroundStyle(Theme.muted)
        }
        .frame(maxWidth: .infinity, minHeight: 140)
    }

    // MARK: Found tools

    private var choices: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            Text("Found on this Mac").font(Theme.sectionTitle).foregroundStyle(Theme.text)
            VStack(spacing: 0) {
                ForEach(Array(model.detectedRuntimes.enumerated()), id: \.element.id) { index, runtime in
                    if index > 0 { Seam() }
                    ChoiceRow(runtime: runtime, isSelected: selection == runtime.id,
                              isRecommended: runtime.id == model.recommendedRuntime?.id) {
                        selection = runtime.id
                    }
                }
            }
            .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius).strokeBorder(Theme.seam, lineWidth: 1))
            if model.recommendedRuntime == nil {
                Text("Sign in to one of these in Terminal, then choose Check Again.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.secondary)
            }
            if !otherToolNames.isEmpty {
                Text("Tellfold also works with \(otherToolNames). You can change this any time in Settings > Agents.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// Supported tools that are not on this Mac, as a sentence ("Codex, Gemini CLI and Ollama").
    private var otherToolNames: String {
        let found = Set(model.detectedRuntimes.map(\.id))
        let names = RuntimeRecommendation.ranked(model.runtimes)
            .filter { !found.contains($0.id) && $0.id != "grok" && !RuntimeRecommendation.isPayPerToken($0) }
            .map(\.name)
        guard names.count > 1 else { return names.first ?? "" }
        return names.dropLast().joined(separator: ", ") + " and " + names.last!
    }

    // MARK: Nothing installed

    private var installOptions: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            Text("No AI tool was found on this Mac. Install one of these, sign in, then choose Check Again.")
                .font(Theme.body)
                .foregroundStyle(Theme.text)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(RuntimeInstallOption.suggestions) { option in
                InstallCard(option: option) { openURL(option.url) }
            }
            Text("Gemini CLI, Cursor, GitHub Copilot, OpenCode, LM Studio and API keys work too. See Settings > Agents.")
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: Theme.space2) {
            Button("Skip for now") {
                model.skipAgentSetup()
                dismiss()
            }
            .buttonStyle(.consoleGhost)
            .keyboardShortcut(.cancelAction)
            Spacer()
            if isDetecting && !model.detectedRuntimes.isEmpty {
                ProgressView().controlSize(.small)
            }
            if nothingFound {
                Button("Check Again") { Task { await detect() } }
                    .buttonStyle(.consolePrimary)
                    .keyboardShortcut(.defaultAction)
                    .disabled(isDetecting || !model.engine.state.isRunning)
            } else {
                Button("Check Again") { Task { await detect() } }
                    .buttonStyle(.console)
                    .disabled(isDetecting || !model.engine.state.isRunning)
                useButton
            }
        }
        .padding(.horizontal, Theme.space5)
        .padding(.vertical, Theme.space4)
    }

    private var nothingFound: Bool { !isDetecting && model.detectedRuntimes.isEmpty }

    private var useButton: some View {
        Button {
            Task { await choose() }
        } label: {
            if isSaving {
                ProgressView().controlSize(.small)
            } else {
                Text(selectedRuntime.map { "Use \($0.name)" } ?? "Use This Tool")
            }
        }
        .buttonStyle(.consolePrimary)
        .keyboardShortcut(.defaultAction)
        .disabled(!canChoose)
    }

    // MARK: Logic

    private var selectedRuntime: RuntimeInfo? {
        model.detectedRuntimes.first { $0.id == selection }
    }

    private var canChoose: Bool {
        guard let selectedRuntime, !isSaving, !isDetecting else { return false }
        return RuntimeRecommendation.readiness(selectedRuntime) == .ready
    }

    private func detect() async {
        guard !isDetecting else { return }
        isDetecting = true
        await model.detectAgents()
        isDetecting = false
        if selectedRuntime.map({ RuntimeRecommendation.readiness($0) != .ready }) ?? true {
            selection = model.recommendedRuntime?.id
        }
    }

    private func choose() async {
        guard let id = selection, canChoose else { return }
        isSaving = true
        await model.useRuntime(id)
        isSaving = false
        dismiss()
    }
}

/// One detected tool: a radio mark, its name and a status line. Tools that cannot run yet are shown but not selectable.
private struct ChoiceRow: View {
    let runtime: RuntimeInfo
    let isSelected: Bool
    let isRecommended: Bool
    let select: () -> Void

    private var isReady: Bool { RuntimeRecommendation.readiness(runtime) == .ready }

    /// Hints carry commands in backticks (`` `cursor-agent login` ``); show them as code, not as literal backticks.
    static func inlineCode(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }

    var body: some View {
        Button(action: select) {
            HStack(alignment: .firstTextBaseline, spacing: Theme.space3) {
                Image(systemName: isSelected ? "largecircle.fill.circle" : "circle")
                    .font(.system(size: 14))
                    .foregroundStyle(isSelected ? Theme.accent : (isReady ? Theme.muted : Theme.faint))
                    .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 3 }
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: Theme.space2) {
                        Text(runtime.name)
                            .font(Theme.bodyStrong)
                            .foregroundStyle(isReady ? Theme.text : Theme.muted)
                        if isRecommended {
                            Text("Recommended")
                                .font(Theme.ui(11, .medium))
                                .foregroundStyle(Theme.accent)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 1)
                                .background(Theme.accentSoft, in: Capsule())
                        }
                        Spacer(minLength: Theme.space2)
                        Image(systemName: runtime.isLocal ? (runtime.kind == .cli ? "terminal" : "desktopcomputer") : "network")
                            .font(.system(size: 11))
                            .foregroundStyle(Theme.faint)
                            .help(runtime.kind == .cli ? "Command-line tool" : (runtime.isLocal ? "Local server" : "Online API"))
                            .accessibilityHidden(true)
                    }
                    Text(Self.inlineCode(RuntimeRecommendation.statusLine(runtime)))
                        .font(Theme.small)
                        .foregroundStyle(isReady ? Theme.secondary : Theme.negative.opacity(0.85))
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
            .padding(.horizontal, Theme.space3)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isSelected ? Theme.accentSoft.opacity(0.6) : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!isReady)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
    }
}

/// Where to get one tool: what it is, the install command to copy, and a link to its page.
private struct InstallCard: View {
    let option: RuntimeInstallOption
    let open: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            HStack(alignment: .firstTextBaseline) {
                Text(option.name).font(Theme.bodyStrong).foregroundStyle(Theme.text)
                Spacer()
                Button("Get \(option.name)", action: open)
                    .buttonStyle(.console(.secondary, compact: true))
            }
            Text(option.blurb)
                .font(Theme.small)
                .foregroundStyle(Theme.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let command = option.command {
                HStack {
                    Text(command)
                        .font(Theme.mono(11))
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
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(Theme.hover, in: RoundedRectangle(cornerRadius: Theme.radius))
            }
        }
        .card(padding: Theme.space3)
    }
}
