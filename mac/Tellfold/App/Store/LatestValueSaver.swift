import Foundation

/// Saves a value through an async call one request at a time, always ending on the newest value.
///
/// `submit` while a save is in flight only replaces the pending value, so rapid edits coalesce and responses can
/// never arrive out of order. Each result is reported with `isLatest`: true when no newer value is waiting, i.e.
/// when the caller may show the result (or, on failure, roll back) without clobbering a newer local edit.
@MainActor
final class LatestValueSaver<Value: Sendable> {
    typealias Save = @MainActor (Value) async throws -> Value

    private let save: Save
    private let onSaved: @MainActor (Value, _ isLatest: Bool) -> Void
    private let onFailed: @MainActor (any Error, _ isLatest: Bool) -> Void
    private var pending: Value?
    private var worker: Task<Void, Never>?

    init(save: @escaping Save,
         onSaved: @escaping @MainActor (Value, _ isLatest: Bool) -> Void,
         onFailed: @escaping @MainActor (any Error, _ isLatest: Bool) -> Void) {
        self.save = save
        self.onSaved = onSaved
        self.onFailed = onFailed
    }

    /// True while a save is running or waiting.
    var isBusy: Bool { worker != nil || pending != nil }

    func submit(_ value: Value) {
        pending = value
        guard worker == nil else { return }
        worker = Task { [weak self] in await self?.drain() }
    }

    func waitUntilIdle() async {
        while let worker {
            await worker.value
        }
    }

    private func drain() async {
        while let value = pending {
            pending = nil
            do {
                let saved = try await save(value)
                onSaved(saved, pending == nil)
            } catch {
                onFailed(error, pending == nil)
            }
        }
        worker = nil
    }
}
