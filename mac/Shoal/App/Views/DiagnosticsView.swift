import AppKit
import SwiftUI

/// Engine state, command line and the live engine log (stdout after the port line, stderr, app notes).
struct DiagnosticsView: View {
    @Environment(AppModel.self) private var model
    @State private var filter = ""
    @State private var followTail = true

    private var engine: EngineController { model.engine }

    private var lines: [EngineLogLine] {
        let q = filter.trimmingCharacters(in: .whitespaces).lowercased()
        return q.isEmpty ? engine.log : engine.log.filter { $0.text.lowercased().contains(q) }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(12)
            Divider()
            logView
        }
        .toolbar {
            ToolbarItemGroup {
                Toggle(isOn: $followTail) {
                    Label("Follow", systemImage: "arrow.down.to.line")
                }
                .help("Keep the newest lines in view")
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(engine.logText, forType: .string)
                } label: {
                    Label("Copy Log", systemImage: "doc.on.doc")
                }
                Button {
                    engine.clearLog()
                } label: {
                    Label("Clear", systemImage: "trash")
                }
                Button {
                    engine.restart()
                } label: {
                    Label("Restart Engine", systemImage: "arrow.clockwise")
                }
            }
        }
        .searchable(text: $filter, prompt: "Filter log")
        .navigationTitle("Diagnostics")
    }

    private var header: some View {
        Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
            GridRow {
                Text("State").foregroundStyle(.secondary)
                Text(stateText)
            }
            GridRow {
                Text("Process").foregroundStyle(.secondary)
                Text(engine.pid.map { "pid \($0)" } ?? "not running").monospacedDigit()
            }
            if let health = engine.health {
                GridRow {
                    Text("Engine").foregroundStyle(.secondary)
                    Text("Shoal \(health.version)\(health.fakeLLM ? " · offline test model" : "")")
                }
            }
            GridRow {
                Text("Runs").foregroundStyle(.secondary)
                Text(model.prefs.resolvedRunsFolder.path).textSelection(.enabled).lineLimit(1).truncationMode(.middle)
            }
            if !engine.commandDescription.isEmpty {
                GridRow {
                    Text("Command").foregroundStyle(.secondary)
                    Text(engine.commandDescription)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .lineLimit(2)
                        .truncationMode(.middle)
                }
            }
        }
        .font(.callout)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var stateText: String {
        if case .failed(let message) = engine.state { return "Failed: \(message)" }
        return engine.state.title
    }

    private var logView: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    ForEach(lines) { line in
                        LogLineRow(line: line)
                            .id(line.id)
                    }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .overlay {
                if lines.isEmpty {
                    ContentUnavailableView(filter.isEmpty ? "No Engine Output Yet" : "No Matching Lines",
                                           systemImage: "text.alignleft")
                }
            }
            .onChange(of: engine.log.last?.id) { _, id in
                guard followTail, let id, filter.isEmpty else { return }
                proxy.scrollTo(id, anchor: .bottom)
            }
            .onAppear {
                if let id = lines.last?.id { proxy.scrollTo(id, anchor: .bottom) }
            }
        }
        .background(Theme.panel)
    }
}

private struct LogLineRow: View {
    let line: EngineLogLine

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(line.date, format: .dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits).second(.twoDigits))
                .foregroundStyle(.tertiary)
            Text(line.source.rawValue)
                .foregroundStyle(line.source == .app ? Theme.accent : Theme.muted)
                .frame(width: 44, alignment: .leading)
            Text(line.text)
                .foregroundStyle(line.source == .stderr && isProblem ? Theme.negative : Theme.text)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(Theme.mono(11))
    }

    private var isProblem: Bool {
        let t = line.text.lowercased()
        return t.contains("error") || t.contains("traceback") || t.contains("exception")
    }
}
