import Foundation
import Observation

/// One study's view state: the `StudyDetail` from `GET /api/studies/{id}`, refreshed every few seconds while a version
/// is running or the comparison is being written.
@MainActor @Observable
final class StudyStore: Identifiable {
    nonisolated let studyID: String
    nonisolated var id: String { studyID }
    @ObservationIgnored private let api: TellfoldAPI?

    private(set) var detail: StudyDetail?
    private(set) var loadError: String?
    private(set) var isLoading = false
    /// A user-facing error from the last action (Compare again).
    var actionError: String?

    @ObservationIgnored private var pollTask: Task<Void, Never>?

    init(studyID: String, api: TellfoldAPI?) {
        self.studyID = studyID
        self.api = api
    }

    /// A store with ready data and no API, for previews and tests.
    init(detail: StudyDetail) {
        studyID = detail.id
        api = nil
        self.detail = detail
    }

    var phase: StudyPhase? { detail.map(StudyLogic.phase(of:)) }
    var letters: Set<String> { Set(detail?.runs.map(\.letter) ?? []) }

    func load() async {
        guard let api else { return }
        isLoading = detail == nil
        defer { isLoading = false }
        do {
            detail = try await api.study(studyID)
            loadError = nil
        } catch is CancellationError {
        } catch {
            loadError = error.localizedDescription
        }
        schedulePoll()
    }

    /// Writes the comparison again (or for the first time, when the automatic one failed).
    func compare() async {
        guard let api else { return }
        do {
            try await api.compareStudy(studyID)
            await load()
        } catch {
            actionError = error.localizedDescription
        }
    }

    func stop() {
        pollTask?.cancel()
        pollTask = nil
    }

    private func schedulePoll() {
        guard pollTask == nil, let phase, phase == .running || phase == .comparing else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                guard !Task.isCancelled, let self else { return }
                self.pollTask = nil
                await self.load()
                return
            }
        }
    }
}
