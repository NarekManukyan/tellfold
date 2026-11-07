import SwiftUI

/// Sidebar: the brand mark and wordmark, New Simulation, search, and runs grouped by state (Running, Done, Stopped,
/// Failed) with plain status chips, live progress and a context menu.
struct RunSidebar: View {
    @Environment(AppModel.self) private var model
    @State private var pendingDelete: RunSummary?
    @State private var errorMessage: String?

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selectedRunID) {
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
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .background(Theme.ground)
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

    /// Runs filtered by the search field, grouped by state: running first, then the ones that need a look.
    private var stateSections: [(title: String, runs: [RunSummary])] {
        let runs = model.groupedRuns.flatMap(\.runs)
        let order: [(String, (RunSummary) -> Bool)] = [
            ("Running", { $0.state.isActive }),
            ("Done", { $0.state == .done }),
            ("Stopped", { $0.state == .stopped }),
            ("Failed", { $0.state == .failed }),
            ("Other", { !$0.state.isActive && ![.failed, .stopped, .done].contains($0.state) }),
        ]
        return order.compactMap { title, matches in
            let group = runs.filter(matches)
            return group.isEmpty ? nil : (title, group)
        }
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

/// Five dots swimming right, then the wordmark.
struct BrandHeader: View {
    var body: some View {
        HStack(spacing: 8) {
            BrandMark()
                .frame(width: 26, height: 16)
            Text("Shoal")
                .font(Theme.ui(15, .semibold))
                .foregroundStyle(Theme.text)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Shoal")
        .accessibilityAddTraits(.isHeader)
    }
}

/// The brand mark: a small school of five dots in the accent colour, in a V that points right.
struct BrandMark: View {
    var body: some View {
        Canvas { ctx, size in
            let sx = size.width / 26, sy = size.height / 16
            // (x, y, radius) in a 26 x 16 box: the lead dot at the right, two pairs trailing behind it.
            let dots: [(Double, Double, Double)] = [(22.5, 8, 2.9), (15.5, 3.8, 2.4), (15.5, 12.2, 2.4), (8.5, 1.9, 1.9), (8.5, 14.1, 1.9)]
            for (x, y, r) in dots {
                let rect = CGRect(x: (x - r) * sx, y: (y - r) * sy, width: 2 * r * sx, height: 2 * r * sy)
                ctx.fill(Path(ellipseIn: rect), with: .color(Theme.accent))
            }
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
                StatusChip(state: run.state, compact: true)
                Text(detail)
                Spacer(minLength: 0)
                if let date = run.createdDate {
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
        .accessibilityLabel("\(run.question), \(run.state.title), round \(roundsDone) of \(run.totalRounds)")
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
            Seam()
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
                        .help("The engine uses canned offline answers (SHOAL_FAKE_LLM)")
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
        .background(Theme.ground)
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
