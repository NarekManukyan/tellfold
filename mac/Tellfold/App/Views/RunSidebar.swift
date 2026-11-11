import SwiftUI

/// Sidebar: the brand mark and wordmark, New Simulation, search, and runs grouped by state (Running, Done, Stopped,
/// Failed) with plain status chips, live progress and a context menu. The built-in example runs come last, in their
/// own Examples section.
struct RunSidebar: View {
    @Environment(AppModel.self) private var model
    @State private var pendingDelete: RunSummary?
    @State private var errorMessage: String?

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selectedRunID) {
            if !studyRows.isEmpty {
                Section {
                    ForEach(studyRows) { study in
                        StudyRowView(study: study)
                            .tag(StudySelection.tag(study.id))
                    }
                } header: {
                    HStack {
                        Text("Studies")
                            .font(Theme.ui(12, .semibold))
                            .foregroundStyle(Theme.muted)
                        Spacer()
                        Text("\(studyRows.count)").font(Theme.small).foregroundStyle(Theme.faint)
                    }
                    .padding(.trailing, 8)
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(.isHeader)
                }
            }
            ForEach(stateSections, id: \.title) { section in
                Section {
                    ForEach(section.runs) { run in
                        RunRow(run: run, roundsDone: model.roundsDone(run))
                            .tag(run.id)
                            .contextMenu { contextMenu(for: run) }
                    }
                } header: {
                    HStack {
                        Text(section.title)
                            .font(Theme.ui(12, .semibold))
                            .foregroundStyle(Theme.muted)
                        Spacer()
                        Text("\(section.runs.count)").font(Theme.small).foregroundStyle(Theme.faint)
                    }
                    .padding(.trailing, 8)
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(.isHeader)
                }
            }
        }
        // No custom background: the system's floating Liquid Glass sidebar draws itself.
        .listStyle(.sidebar)
        .overlay { emptyState }
        .safeAreaInset(edge: .top, spacing: 0) {
            VStack(alignment: .leading, spacing: Theme.space3) {
                BrandHeader()
                Button {
                    model.newSimulation()
                } label: {
                    Label("New simulation", systemImage: "plus")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(ConsoleButtonStyle(kind: .primary))
                .disabled(!model.engine.state.isRunning)
                .help("Start a new simulation (⌘N)")
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(Theme.muted)
                    TextField("Search simulations", text: $model.searchText)
                        .textFieldStyle(.plain)
                        .font(Theme.body)
                        .accessibilityLabel("Search simulations")
                    if !model.searchText.isEmpty {
                        Button { model.searchText = "" } label: {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.faint)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Clear search")
                    }
                }
                .padding(.horizontal, 8)
                .frame(height: 28)
                .background(Theme.hover, in: RoundedRectangle(cornerRadius: Theme.radius))
            }
            .padding(.horizontal, 14)
            .padding(.top, 6)
            .padding(.bottom, 10)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            EngineStatusBar()
        }
        .confirmationDialog("Move this simulation to the Trash?", isPresented: deleteBinding, presenting: pendingDelete) { run in
            Button("Move to Trash", role: .destructive) {
                Task {
                    do { try await model.deleteRun(run.id) } catch { errorMessage = error.localizedDescription }
                }
            }
        } message: { run in
            Text(run.question)
        }
        .alert("Something Went Wrong", isPresented: Binding(get: { errorMessage != nil }, set: { if !$0 { errorMessage = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(errorMessage ?? "")
        }
    }

    /// Studies whose question matches the search field.
    private var studyRows: [StudyRow] {
        let query = model.searchText.trimmingCharacters(in: .whitespaces).lowercased()
        return model.studyRows.filter { query.isEmpty || $0.question.lowercased().contains(query) }
    }

    /// Runs filtered by the search field, grouped by state: running first, then the ones that need a look.
    private var stateSections: [(title: String, runs: [RunSummary])] {
        Self.sections(for: model.groupedRuns.flatMap(\.runs))
    }

    static let examplesTitle = "Examples"

    /// The user's runs grouped by state, then the built-in examples on their own. Empty sections are left out.
    static func sections(for all: [RunSummary]) -> [(title: String, runs: [RunSummary])] {
        let runs = all.filter { !$0.example }
        let examples = all.filter(\.example)
        let order: [(String, (RunSummary) -> Bool)] = [
            ("Running", { $0.state.isActive }),
            ("Done", { $0.state == .done }),
            ("Stopped", { $0.state == .stopped }),
            ("Failed", { $0.state == .failed }),
            ("Other", { !$0.state.isActive && ![.failed, .stopped, .done].contains($0.state) }),
        ]
        let byState: [(title: String, runs: [RunSummary])] = order.compactMap { title, matches in
            let group = runs.filter(matches)
            return group.isEmpty ? nil : (title, group)
        }
        return byState + (examples.isEmpty ? [] : [(examplesTitle, examples)])
    }

    @ViewBuilder private var emptyState: some View {
        if model.engine.state.isRunning, model.hasLoadedRuns, model.groupedRuns.isEmpty {
            if model.searchText.isEmpty {
                ContentUnavailableView("No Simulations", systemImage: "tray",
                                       description: Text("New simulations appear here."))
            } else {
                ContentUnavailableView.search(text: model.searchText)
            }
        } else if model.engine.state.isRunning, !model.hasLoadedRuns {
            if let error = model.runsError {
                ContentUnavailableView("Could Not Load Simulations", systemImage: "exclamationmark.triangle",
                                       description: Text(error))
            } else {
                ProgressView()
            }
        }
    }

    @ViewBuilder private func contextMenu(for run: RunSummary) -> some View {
        if run.example {
            Button("Open HTML Report") {
                Task {
                    do { try await model.openHTMLReport(run.id) } catch { errorMessage = error.localizedDescription }
                }
            }
            .disabled(!run.hasReport)
        } else {
            runContextMenu(for: run)
        }
    }

    @ViewBuilder private func runContextMenu(for run: RunSummary) -> some View {
        Button("Reveal in Finder") { model.revealInFinder(run.id) }
        Button("Open HTML Report") {
            Task {
                do { try await model.openHTMLReport(run.id) } catch { errorMessage = error.localizedDescription }
            }
        }
        .disabled(!run.hasReport)
        Button("Rebuild Report") {
            Task {
                do { try await model.rebuildReport(run.id) } catch { errorMessage = error.localizedDescription }
            }
        }
        .disabled(run.state.isActive || !model.engine.state.isRunning)
        Divider()
        Button("Move to Trash…", role: .destructive) { pendingDelete = run }
            .disabled(run.state.isActive)
    }

    private var deleteBinding: Binding<Bool> {
        Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } })
    }
}

/// A study in the sidebar: its question and its versions ("Draft A vs Draft B").
private struct StudyRowView: View {
    let study: StudyRow

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(study.question)
                .lineLimit(2)
                .font(Theme.ui(13, .medium))
                .foregroundStyle(Theme.text)
            HStack(spacing: 6) {
                Image(systemName: "square.split.2x1")
                    .foregroundStyle(Theme.accent)
                Text(study.title).lineLimit(1)
                Spacer(minLength: 0)
                if study.isActive || study.comparing {
                    ProgressView().controlSize(.mini)
                }
            }
            .font(Theme.small)
            .foregroundStyle(Theme.muted)
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Study: \(study.question), \(study.title)")
    }
}

/// The folding-fan mark, then the wordmark.
struct BrandHeader: View {
    var body: some View {
        HStack(spacing: 8) {
            BrandMark()
                .frame(width: 18, height: 18)
            Text("Tellfold")
                .font(Theme.ui(15, .semibold))
                .foregroundStyle(Theme.text)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Tellfold")
        .accessibilityAddTraits(.isHeader)
    }
}

/// The brand mark: a folding fan whose four pleats (the crowd) fold into one hinge (the answer).
/// The small-size cut of `brand/tellfold-symbol-small.svg`, on its 196-unit box.
struct BrandMark: View {
    @Environment(\.colorScheme) private var colorScheme

    private static let hinge = CGPoint(x: 24, y: 168)
    /// Outer corners of the pleats, from the flat bottom pleat to the upright one.
    private static let rim: [CGPoint] = [
        CGPoint(x: 184, y: 168), CGPoint(x: 171.82, y: 106.77), CGPoint(x: 137.14, y: 54.86),
        CGPoint(x: 85.23, y: 20.18), CGPoint(x: 24, y: 8),
    ]

    var body: some View {
        Canvas { ctx, size in
            let s = min(size.width, size.height) / 196
            func pt(_ p: CGPoint) -> CGPoint { CGPoint(x: p.x * s, y: p.y * s) }
            let light = Color(red: 0x63 / 255, green: 0x66 / 255, blue: 0xF1 / 255)
            let dark = Color(red: 0x43 / 255, green: 0x38 / 255, blue: 0xCA / 255)
            for i in 0..<4 {
                var pleat = Path()
                pleat.move(to: pt(Self.hinge))
                pleat.addLine(to: pt(Self.rim[i]))
                pleat.addLine(to: pt(Self.rim[i + 1]))
                pleat.closeSubpath()
                ctx.fill(pleat, with: .color(i.isMultiple(of: 2) ? light : dark))
            }
            let r = 22 * s
            let c = pt(Self.hinge)
            let hingeColor = colorScheme == .dark
                ? Color(red: 0xA5 / 255, green: 0xB4 / 255, blue: 0xFC / 255)
                : Color(red: 0x31 / 255, green: 0x2E / 255, blue: 0x81 / 255)
            ctx.fill(Path(ellipseIn: CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r)), with: .color(hingeColor))
        }
        .accessibilityHidden(true)
    }
}

private struct RunRow: View {
    let run: RunSummary
    /// Rounds the run completed by its recorded data (see `AppModel.roundsDone`): wins over a status that lags.
    let roundsDone: Int

    /// "r3 of 6" for a run that did not finish, the size of the crowd for one that did.
    private var detail: String {
        if run.awaitsCast { return "waiting for you to check the cast" }
        if run.state.isActive { return "\(run.stage.title.lowercased()), round \(run.round) of \(run.totalRounds)" }
        if run.state == .stopped || run.state == .failed { return "r\(roundsDone) of \(run.totalRounds)" }
        return "\(run.agents) agents, \(run.totalRounds) rounds"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(run.question.isEmpty ? run.id : run.question)
                .lineLimit(2)
                .font(Theme.ui(13, .medium))
                .foregroundStyle(Theme.text)
            HStack(spacing: 6) {
                if run.example {
                    ExampleBadge()
                } else {
                    StatusChip(state: run.state, compact: true)
                }
                Text(detail)
                Spacer(minLength: 0)
                if !run.example, let date = run.createdDate {
                    Text(date, format: .relative(presentation: .numeric, unitsStyle: .abbreviated))
                        .foregroundStyle(Theme.faint)
                }
            }
            .font(Theme.ui(11))
            .foregroundStyle(Theme.muted)
            .lineLimit(1)
            if run.state.isActive {
                ProgressBar(value: run.progress).frame(height: 3)
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(run.example
            ? "\(run.question), example run"
            : "\(run.question), \(run.state.title), round \(roundsDone) of \(run.totalRounds)")
    }
}

/// The small "Example" tag on a built-in example run.
struct ExampleBadge: View {
    var body: some View {
        Text("Example")
            .font(Theme.ui(10, .semibold))
            .foregroundStyle(Theme.accent)
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(Theme.accentSoft, in: Capsule())
            .help("A finished example run that ships with Tellfold. It is read-only; Run again makes your own copy.")
    }
}

/// A thin rounded progress bar.
struct ProgressBar: View {
    let value: Double

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.seam)
                Capsule().fill(Theme.accent).frame(width: max(3, geo.size.width * min(1, max(0, value))))
            }
        }
        .accessibilityHidden(true)
    }
}

/// A one-line engine status at the bottom of the sidebar, with a shortcut to Diagnostics.
struct EngineStatusBar: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Circle()
                    .fill(color)
                    .frame(width: 7, height: 7)
                    .accessibilityHidden(true)
                Text(title)
                    .font(Theme.small)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
                if model.engine.health?.fakeLLM == true {
                    Text("Offline model")
                        .font(Theme.ui(11, .medium))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(Theme.hover, in: Capsule())
                        .help("The engine uses canned offline answers (TELLFOLD_FAKE_LLM)")
                }
                Button {
                    openWindow(id: "diagnostics")
                } label: {
                    Image(systemName: "stethoscope")
                }
                .buttonStyle(.plain)
                .help("Open Diagnostics (⌥⌘D)")
                .accessibilityLabel("Open Diagnostics")
            }
            .foregroundStyle(Theme.muted)
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
        }
        .accessibilityElement(children: .contain)
    }

    private var title: String {
        switch model.engine.state {
        case .running: "Engine ready"
        case .starting: "Starting engine…"
        case .waitingToRestart(_, let s): "Engine restarting in \(s) s"
        case .failed: "Engine unavailable"
        case .uvMissing: "uv not installed"
        case .stopped: "Engine stopped"
        }
    }

    private var color: Color {
        switch model.engine.state {
        case .running: Theme.positive
        case .starting, .waitingToRestart: Theme.muted
        case .failed, .uvMissing: Theme.negative
        case .stopped: Theme.muted
        }
    }
}
