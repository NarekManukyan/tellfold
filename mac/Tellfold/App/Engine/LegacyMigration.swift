import Foundation
import os

/// One-time carry-over from the app's previous name (Shoal): preferences, the Application Support folder, and API
/// keys in the Keychain. It runs once, early at launch, before `AppPreferences` or the engine read anything
/// (see `AppDelegate.makeModel`). Nothing is deleted: the old defaults domain and Keychain items stay as they were.
///
/// Every location is injected so tests never touch the real user's data.
struct LegacyMigration {
    static let legacyDefaultsDomain = "dev.shoal.mac"
    static let legacyKeychainService = "dev.shoal.mac.api-keys"
    static let legacyFolderName = "Shoal"
    static let folderName = "Tellfold"
    /// Set in the new defaults once the migration has run.
    static let doneKey = "legacyMigrationDone"

    private static let log = Logger(subsystem: "dev.tellfold.mac", category: "LegacyMigration")

    /// What a run did; used by tests and the log.
    struct Report: Equatable {
        var skipped = false
        var movedFolder = false
        var copiedDefaultsKeys: [String] = []
        var rewroteRunsFolder = false
        var copiedKeychainItems = 0
    }

    /// The defaults the app now uses, and the name of the persistent domain they write to (the bundle id for
    /// `.standard`). Only keys missing from that domain are filled in.
    var defaults: UserDefaults
    var defaultsDomain: String
    /// Defaults object that can read `legacyDefaultsDomain` (or, in tests, a stand-in suite) and its domain name.
    var legacyDefaults: UserDefaults
    var legacyDomain: String
    /// `~/Library/Application Support`.
    var applicationSupport: URL
    var fileManager: FileManager
    var keychain: Keychain
    var legacyKeychain: Keychain

    /// The real locations of the current user.
    static func live() -> LegacyMigration {
        LegacyMigration(
            defaults: .standard,
            defaultsDomain: Bundle.main.bundleIdentifier ?? "dev.tellfold.mac",
            legacyDefaults: UserDefaults(suiteName: legacyDefaultsDomain) ?? .standard,
            legacyDomain: legacyDefaultsDomain,
            applicationSupport: URL.applicationSupportDirectory,
            fileManager: .default,
            keychain: .apiKeys,
            legacyKeychain: Keychain(service: legacyKeychainService))
    }

    /// Runs the migration unless it already ran. Failures are logged and never thrown: the app must start anyway.
    @discardableResult
    func runIfNeeded() -> Report {
        guard !defaults.bool(forKey: Self.doneKey) else { return Report(skipped: true) }
        var report = Report()
        var finished = true

        // 1. The Application Support folder first, so stored paths can be checked against what is on disk.
        do { report.movedFolder = try moveSupportFolder() } catch {
            Self.log.error("Could not move the Application Support folder: \(error.localizedDescription, privacy: .public)")
            finished = false
        }

        // 2. Preferences, then the runs folder path that may point into the old folder.
        report.copiedDefaultsKeys = copyDefaults()
        report.rewroteRunsFolder = rewriteRunsFolder()

        // 3. API keys.
        report.copiedKeychainItems = legacyKeychain.service == keychain.service ? 0 : keychain.copyItemsIfEmpty(from: legacyKeychain)

        if finished { defaults.set(true, forKey: Self.doneKey) }
        Self.log.info("Migrated: folder \(report.movedFolder), \(report.copiedDefaultsKeys.count) preferences, \(report.copiedKeychainItems) keys")
        return report
    }

    // MARK: Folder

    private var oldFolder: URL { applicationSupport.appending(path: Self.legacyFolderName, directoryHint: .isDirectory) }
    private var newFolder: URL { applicationSupport.appending(path: Self.folderName, directoryHint: .isDirectory) }

    /// Renames `Application Support/Shoal` to `Tellfold` when only the old one exists. The engine does the same on
    /// its side, so a folder that is already moved is not an error. Returns whether this call moved it.
    func moveSupportFolder() throws -> Bool {
        guard fileManager.fileExists(atPath: oldFolder.path), !fileManager.fileExists(atPath: newFolder.path) else { return false }
        do {
            try fileManager.moveItem(at: oldFolder, to: newFolder)
            return true
        } catch {
            // The engine may have won the race between the check and the move.
            if fileManager.fileExists(atPath: newFolder.path), !fileManager.fileExists(atPath: oldFolder.path) { return false }
            throw error
        }
    }

    // MARK: Preferences

    /// Copies every key of the old domain that is not a system (`NS`/`Apple`) key and not yet set in the new domain.
    func copyDefaults() -> [String] {
        guard let old = legacyDefaults.persistentDomain(forName: legacyDomain) else { return [] }
        let existing = defaults.persistentDomain(forName: defaultsDomain) ?? [:]
        var copied: [String] = []
        for (key, value) in old.sorted(by: { $0.key < $1.key }) {
            guard !key.hasPrefix("NS"), !key.hasPrefix("Apple"), existing[key] == nil else { continue }
            defaults.set(value, forKey: key)
            copied.append(key)
        }
        return copied
    }

    /// Points a stored `runsFolder` that lives inside the old support folder at the new one. Only done when the
    /// old folder is gone, so a run folder that still exists where it was is never redirected to an empty one.
    func rewriteRunsFolder() -> Bool {
        guard let stored = defaults.string(forKey: "runsFolder"),
              !fileManager.fileExists(atPath: oldFolder.path),
              let rewritten = Self.rewrite(path: stored), rewritten != stored else { return false }
        defaults.set(rewritten, forKey: "runsFolder")
        return true
    }

    /// `…/Application Support/Shoal[/…]` → `…/Application Support/Tellfold[/…]`; nil when the path is elsewhere.
    static func rewrite(path: String) -> String? {
        let marker = "Application Support/" + legacyFolderName
        var searchStart = path.startIndex
        while let range = path.range(of: marker, range: searchStart..<path.endIndex) {
            let atBoundary = range.upperBound == path.endIndex || path[range.upperBound] == "/"
            if atBoundary {
                return path.replacingCharacters(in: range, with: "Application Support/" + folderName)
            }
            searchStart = range.upperBound
        }
        return nil
    }
}

extension Keychain {
    /// When this service has no items and `legacy` has some, copies each one over (account, secret, label). The
    /// legacy items are left in place. Returns how many were copied.
    @discardableResult
    func copyItemsIfEmpty(from legacy: Keychain) -> Int {
        guard accounts().isEmpty else { return 0 }
        var copied = 0
        for account in legacy.accounts() {
            guard let value = legacy.string(for: account), (try? set(value, for: account)) != nil else { continue }
            copied += 1
        }
        return copied
    }
}
