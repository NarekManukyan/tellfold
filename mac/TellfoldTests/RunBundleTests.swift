import Foundation
import Testing
import UniformTypeIdentifiers
@testable import Tellfold

// `.tellfold` run bundles: the app fetches a finished run as one file, sends a chosen file to the engine to open, and
// treats the imported run like an example: read-only, marked in the sidebar.
@MainActor
@Suite("Run bundles")
struct RunBundleTests {
    private static func folder() throws -> URL {
        let url = URL.temporaryDirectory.appending(path: "tellfold-bundle-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    @Test func exportDownloadsTheBundleToTheChosenFile() async throws {
        let zip = Data("PK\u{3}\u{4}bundle".utf8)
        let (session, key) = StubURLProtocol.session { _ in .init(headers: ["Content-Type": RunBundle.mediaType], body: zip) }
        let api = TellfoldAPI(port: 7, token: "secret", session: session)
        let folder = try Self.folder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let file = folder.appending(path: "Shared.tellfold")
        try Data("an older file".utf8).write(to: file)
        try await api.downloadBundle(runID: "20261110-090604-x", to: file)
        #expect(try Data(contentsOf: file) == zip, "the file there is replaced")
        let request = try #require(StubURLProtocol.requests(key).first)
        #expect(request.url?.absoluteString == "http://127.0.0.1:7/api/runs/20261110-090604-x/bundle")
        #expect(request.httpMethod == "GET")
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer secret")
    }

    @Test func aRefusedExportLeavesNoFileAndSaysWhy() async throws {
        let detail = Data(#"{"detail":"only a finished run with a report can be shared"}"#.utf8)
        let (session, _) = StubURLProtocol.session { _ in .init(status: 400, body: detail) }
        let api = TellfoldAPI(port: 7, token: nil, session: session)
        let folder = try Self.folder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let file = folder.appending(path: "Shared.tellfold")
        await #expect {
            try await api.downloadBundle(runID: "x", to: file)
        } throws: { error in
            (error as? TellfoldAPIError)?.message == "only a finished run with a report can be shared"
        }
        #expect(!FileManager.default.fileExists(atPath: file.path))
    }

    @Test func importSendsTheFileAsTheBody() async throws {
        let bytes = Data("PK\u{3}\u{4}a whole run".utf8)
        let folder = try Self.folder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let file = folder.appending(path: "Their run.tellfold")
        try bytes.write(to: file)
        let (session, key) = StubURLProtocol.session { _ in .init(body: Data(#"{"id":"20261110-090604-x-2"}"#.utf8)) }
        let api = TellfoldAPI(port: 7, token: nil, session: session)
        #expect(try await api.importBundle(fileAt: file) == "20261110-090604-x-2")
        let request = try #require(StubURLProtocol.requests(key).first)
        #expect(request.url?.absoluteString == "http://127.0.0.1:7/api/runs/import")
        #expect(request.httpMethod == "POST")
        #expect(request.value(forHTTPHeaderField: "Content-Type") == RunBundle.mediaType)
        #expect(request.httpBody == bytes)
    }

    @Test func aRefusedBundleSaysWhy() async throws {
        let detail = Data(#"{"detail":"the bundle holds an unsafe name: \"../x\""}"#.utf8)
        let (session, _) = StubURLProtocol.session { _ in .init(status: 400, body: detail) }
        let api = TellfoldAPI(port: 7, token: nil, session: session)
        let folder = try Self.folder()
        defer { try? FileManager.default.removeItem(at: folder) }
        let file = folder.appending(path: "bad.tellfold")
        try Data("PK".utf8).write(to: file)
        await #expect {
            try await api.importBundle(fileAt: file)
        } throws: { error in
            (error as? TellfoldAPIError)?.message.contains("unsafe name") == true
        }
    }

    @Test func importedRunsDecodeAsReadOnly() throws {
        let json = #"""
            [{"id":"a","question":"Q","state":"done","stage":"finished","round":2,"total_rounds":2,"agents":4,
              "created_at":"2026-11-10T05:06:04Z","has_report":true,"example":false,"imported":true},
             {"id":"b","question":"Q","state":"done","stage":"finished","round":2,"total_rounds":2,"agents":4,
              "created_at":"2026-11-10T05:06:04Z","has_report":true,"example":false}]
            """#
        let runs = try TellfoldAPI.decoder.decode([RunSummary].self, from: Data(json.utf8))
        #expect(runs.map(\.imported) == [true, false])
        // They sit with the user's own runs (marked), not with the examples.
        #expect(RunSidebar.sections(for: runs).map(\.title) == ["Done"])

        var detail = try Fixtures.decode(RunDetail.self, "run_detail.json")
        #expect(!detail.imported)
        detail.imported = true
        let store = RunStore(detail: detail, posts: [], actions: [])
        #expect(store.isImported && store.isReadOnly && !store.isExample)
        #expect(store.controls == RunControls(isLive: false, isReadOnly: true))
        #expect(!store.controls.rebuildReport && !store.controls.injectEvent)
        #expect(store.canRunAgain)
    }

    @Test func studyRunsSayWhenTheyAreImported() throws {
        let json = #"[{"id":"a","variant":"Original","letter":"A","imported":true},{"id":"b","variant":"B","letter":"B"}]"#
        let runs = try TellfoldAPI.decoder.decode([StudyRun].self, from: Data(json.utf8))
        #expect(runs.map(\.imported) == [true, false])
    }

    @Test func filesAndNames() throws {
        #expect(RunBundle.isBundle(URL(filePath: "/tmp/Run.TELLFOLD")))
        #expect(!RunBundle.isBundle(URL(filePath: "/tmp/notes.md")))
        #expect(RunBundle.suggestedFileName(title: "Plus: yes/no?") == "Plus yes no?.tellfold")
        #expect(RunBundle.suggestedFileName(title: "  ") == "Tellfold run.tellfold")
        #expect(UTType.tellfoldBundle.identifier == "dev.tellfold.run-bundle")
        #expect(UTType.tellfoldBundle.conforms(to: .zip))
        // The app declares the type, so Finder hands `.tellfold` files to it.
        let declared = try #require(Bundle.main.object(forInfoDictionaryKey: "UTExportedTypeDeclarations") as? [[String: Any]])
        let tags = declared.first { $0["UTTypeIdentifier"] as? String == "dev.tellfold.run-bundle" }?["UTTypeTagSpecification"]
        #expect((tags as? [String: Any])?["public.filename-extension"] as? [String] == ["tellfold"])
        let documents = try #require(Bundle.main.object(forInfoDictionaryKey: "CFBundleDocumentTypes") as? [[String: Any]])
        #expect(documents.contains { ($0["LSItemContentTypes"] as? [String]) == ["dev.tellfold.run-bundle"] })
    }

    @Test func aTooLargeFileIsRefusedBeforeSending() throws {
        let url = URL.temporaryDirectory.appending(path: "big-\(UUID().uuidString).tellfold")
        FileManager.default.createFile(atPath: url.path, contents: nil)
        defer { try? FileManager.default.removeItem(at: url) }
        let handle = try FileHandle(forWritingTo: url)
        try handle.truncate(atOffset: UInt64(RunBundle.maxBytes) + 1)
        try handle.close()
        #expect(throws: RunBundle.FileError.self) { try RunBundle.checkReadable(url) }
        let missing = URL.temporaryDirectory.appending(path: "missing-\(UUID().uuidString).tellfold")
        #expect(throws: RunBundle.FileError.self) { try RunBundle.checkReadable(missing) }
    }

    @Test func fileNamesShareTheReportsBaseName() {
        #expect(ReportPDFExporter.baseName(title: " a/b: c. ") == "a b c")
        #expect(ReportPDFExporter.suggestedFileName(title: "") == "Tellfold report.pdf")
        #expect(RunBundle.suggestedFileName(title: "Plus: yes") == "Plus yes.tellfold")
        #expect(ShareCardRenderer.suggestedFileName(title: "", format: .landscape, scheme: .light)
            == "Tellfold report (landscape, light).png")
    }
}
