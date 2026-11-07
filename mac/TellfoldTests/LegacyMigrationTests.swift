import Foundation
import Testing
@testable import Tellfold

/// The Shoal -> Tellfold carry-over, against throwaway defaults suites, a temp Application Support folder and
/// uniquely named Keychain services.
@Suite(.serialized)
struct LegacyMigrationTests {
    private final class Sandbox {
        let tempRoot = URL.temporaryDirectory.appending(path: "tellfold-migration-\(UUID().uuidString)", directoryHint: .isDirectory)
        /// Stands in for `~/Library/Application Support`.
        var root: URL { tempRoot.appending(path: "Application Support", directoryHint: .isDirectory) }
        let newSuite = "dev.tellfold.mac.tests.new.\(UUID().uuidString)"
        let oldSuite = "dev.tellfold.mac.tests.old.\(UUID().uuidString)"
        let newKeychain = Keychain(service: "dev.tellfold.mac.tests.kc-new.\(UUID().uuidString)")
        let oldKeychain = Keychain(service: "dev.tellfold.mac.tests.kc-old.\(UUID().uuidString)")
        let defaults: UserDefaults
        let oldDefaults: UserDefaults

        init() throws {
            defaults = try #require(UserDefaults(suiteName: newSuite))
            oldDefaults = try #require(UserDefaults(suiteName: oldSuite))
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        }

        deinit {
            try? FileManager.default.removeItem(at: tempRoot)
            defaults.removePersistentDomain(forName: newSuite)
            oldDefaults.removePersistentDomain(forName: oldSuite)
            for account in newKeychain.accounts() { try? newKeychain.delete(account) }
            for account in oldKeychain.accounts() { try? oldKeychain.delete(account) }
        }

        var oldFolder: URL { root.appending(path: "Shoal", directoryHint: .isDirectory) }
        var newFolder: URL { root.appending(path: "Tellfold", directoryHint: .isDirectory) }

        var migration: LegacyMigration {
            LegacyMigration(defaults: defaults, defaultsDomain: newSuite, legacyDefaults: oldDefaults, legacyDomain: oldSuite,
                            applicationSupport: root, fileManager: .default, keychain: newKeychain, legacyKeychain: oldKeychain)
        }

        func makeOldFolder() throws {
            try FileManager.default.createDirectory(at: oldFolder.appending(path: "runs"), withIntermediateDirectories: true)
            try Data("x".utf8).write(to: oldFolder.appending(path: "runs/marker.txt"))
        }
    }

    @Test func copiesPreferencesThatAreNotSetYet() throws {
        let box = try Sandbox()
        box.oldDefaults.set("/Volumes/ext/runs", forKey: "runsFolder")
        box.oldDefaults.set("/eng", forKey: "enginePath")
        box.oldDefaults.set(true, forKey: "offlineTestModel")
        box.oldDefaults.set(true, forKey: "askedNotificationPermission")
        box.oldDefaults.set(true, forKey: "newSimulation.advancedOpen")
        box.oldDefaults.set("en", forKey: "NSLanguage")
        box.defaults.set("/mine", forKey: "enginePath") // already chosen in the new app

        let report = box.migration.runIfNeeded()

        #expect(box.defaults.string(forKey: "runsFolder") == "/Volumes/ext/runs")
        #expect(box.defaults.string(forKey: "enginePath") == "/mine")
        #expect(box.defaults.bool(forKey: "offlineTestModel"))
        #expect(box.defaults.bool(forKey: "askedNotificationPermission"))
        #expect(box.defaults.bool(forKey: "newSimulation.advancedOpen"))
        #expect(box.defaults.persistentDomain(forName: box.newSuite)?["NSLanguage"] == nil)
        #expect(!report.copiedDefaultsKeys.contains("enginePath"))
        #expect(!report.copiedDefaultsKeys.contains("NSLanguage"))
        #expect(box.oldDefaults.string(forKey: "runsFolder") == "/Volumes/ext/runs") // the old domain is untouched
    }

    @Test func runsOnlyOnce() throws {
        let box = try Sandbox()
        box.oldDefaults.set("a", forKey: "enginePath")
        #expect(box.migration.runIfNeeded().copiedDefaultsKeys == ["enginePath"])
        #expect(box.defaults.bool(forKey: LegacyMigration.doneKey))

        box.defaults.removeObject(forKey: "enginePath")
        box.oldDefaults.set("b", forKey: "uvPath")
        let second = box.migration.runIfNeeded()
        #expect(second.skipped)
        #expect(box.defaults.string(forKey: "enginePath") == nil)
        #expect(box.defaults.string(forKey: "uvPath") == nil)
    }

    @Test func movesTheSupportFolderAndRewritesTheRunsFolder() throws {
        let box = try Sandbox()
        try box.makeOldFolder()
        let oldRuns = box.oldFolder.appending(path: "runs").path
        box.oldDefaults.set(oldRuns, forKey: "runsFolder")

        let report = box.migration.runIfNeeded()

        #expect(report.movedFolder)
        #expect(report.rewroteRunsFolder)
        #expect(!FileManager.default.fileExists(atPath: box.oldFolder.path))
        #expect(FileManager.default.fileExists(atPath: box.newFolder.appending(path: "runs/marker.txt").path))
        #expect(box.defaults.string(forKey: "runsFolder") == box.newFolder.appending(path: "runs").path)
    }

    @Test func toleratesTheEngineHavingMovedTheFolderAlready() throws {
        let box = try Sandbox()
        try FileManager.default.createDirectory(at: box.newFolder.appending(path: "runs"), withIntermediateDirectories: true)
        box.oldDefaults.set(box.oldFolder.appending(path: "runs").path, forKey: "runsFolder")

        let report = box.migration.runIfNeeded()

        #expect(!report.movedFolder)
        #expect(report.rewroteRunsFolder)
        #expect(box.defaults.string(forKey: "runsFolder") == box.newFolder.appending(path: "runs").path)
        #expect(box.defaults.bool(forKey: LegacyMigration.doneKey))
    }

    @Test func leavesBothFoldersAndTheStoredPathAloneWhenBothExist() throws {
        let box = try Sandbox()
        try box.makeOldFolder()
        try FileManager.default.createDirectory(at: box.newFolder, withIntermediateDirectories: true)
        let oldRuns = box.oldFolder.appending(path: "runs").path
        box.oldDefaults.set(oldRuns, forKey: "runsFolder")

        let report = box.migration.runIfNeeded()

        #expect(!report.movedFolder)
        #expect(!report.rewroteRunsFolder)
        #expect(FileManager.default.fileExists(atPath: box.oldFolder.appending(path: "runs/marker.txt").path))
        #expect(box.defaults.string(forKey: "runsFolder") == oldRuns)
    }

    @Test func doesNothingOnAFreshInstall() throws {
        let box = try Sandbox()
        let report = box.migration.runIfNeeded()
        #expect(report == LegacyMigration.Report())
        #expect(!FileManager.default.fileExists(atPath: box.newFolder.path))
        #expect(box.defaults.bool(forKey: LegacyMigration.doneKey))
    }

    @Test func runsFolderRewriteOnlyTouchesTheSupportFolder() {
        let rewrite = LegacyMigration.rewrite(path:)
        #expect(rewrite("/Users/me/Library/Application Support/Shoal/runs") == "/Users/me/Library/Application Support/Tellfold/runs")
        #expect(rewrite("/Users/me/Library/Application Support/Shoal") == "/Users/me/Library/Application Support/Tellfold")
        #expect(rewrite("~/Library/Application Support/Shoal/runs/x") == "~/Library/Application Support/Tellfold/runs/x")
        #expect(rewrite("/Users/me/Library/Application Support/Shoal2/runs") == nil)
        #expect(rewrite("/Volumes/ext/Shoal/runs") == nil)
        #expect(rewrite("/Users/me/Library/Application Support/Tellfold/runs") == nil)
    }

    @Test func copiesKeychainItemsOnlyIntoAnEmptyService() throws {
        let box = try Sandbox()
        try box.oldKeychain.set("sk-one", for: "ANTHROPIC_API_KEY")
        try box.oldKeychain.set("sk-two", for: "SHOAL_KEY_OR")

        let report = box.migration.runIfNeeded()

        #expect(report.copiedKeychainItems == 2)
        #expect(box.newKeychain.string(for: "ANTHROPIC_API_KEY") == "sk-one")
        #expect(box.newKeychain.string(for: "SHOAL_KEY_OR") == "sk-two")
        #expect(box.oldKeychain.accounts() == ["ANTHROPIC_API_KEY", "SHOAL_KEY_OR"]) // kept

        // A service that already has keys is never overwritten or topped up.
        let other = Keychain(service: "dev.tellfold.mac.tests.kc-other.\(UUID().uuidString)")
        try other.set("mine", for: "OPENAI_API_KEY")
        defer { try? other.delete("OPENAI_API_KEY") }
        #expect(other.copyItemsIfEmpty(from: box.oldKeychain) == 0)
        #expect(other.accounts() == ["OPENAI_API_KEY"])
    }

    @Test func emptyLegacyKeychainIsANoOp() throws {
        let box = try Sandbox()
        #expect(box.migration.runIfNeeded().copiedKeychainItems == 0)
        #expect(box.newKeychain.accounts().isEmpty)
    }

    @Test func legacyCrowdModeRawValueDecodesAsSpectrum() {
        #expect(CrowdMode(rawValue: "shoal") == .spectrum)
        #expect(CrowdMode(rawValue: "spectrum") == .spectrum)
        #expect(CrowdMode(rawValue: "camps") == .camps)
        #expect(CrowdMode(rawValue: "influence") == .influence)
        #expect(CrowdMode(rawValue: "nope") == nil)
        #expect(CrowdMode.spectrum.title == "Spectrum")
    }
}
