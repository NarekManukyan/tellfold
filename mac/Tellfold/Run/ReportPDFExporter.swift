import AppKit
import WebKit

/// Turns a run's `report.html` into a paged PDF: loads it in an offscreen `WKWebView` in the light appearance, prints it
/// to a file with the report's `@media print` rules (page breaks, whole post cards, chart colours), then asks where to
/// save it. `WKWebView.createPDF` is not used because it draws the screen layout as one tall page.
@MainActor
final class ReportPDFExporter: NSObject {
    enum ExportError: LocalizedError {
        case load(String)
        case print

        var errorDescription: String? {
            switch self {
            case .load(let reason): "The report could not be loaded for export: \(reason)"
            case .print: "The report could not be turned into a PDF."
            }
        }
    }

    /// The layout width the report is laid out at before printing; the print rules make it fit the paper.
    private static let viewport = NSRect(x: 0, y: 0, width: 900, height: 1200)

    private let paperSize: NSSize
    private var webView: WKWebView?
    private var window: NSWindow?
    private var loaded: CheckedContinuation<Void, Error>?

    /// `paperSize` defaults to the user's printer paper (A4 or Letter, from the region); margins come from the report's
    /// `@page` rule.
    init(paperSize: NSSize = NSPrintInfo.shared.paperSize) {
        self.paperSize = paperSize
    }

    // MARK: Export

    /// Renders `html`, then saves the PDF where the user picks (default name from `title`). Returns the saved file, or
    /// nil when the user cancels the save panel.
    @discardableResult
    func export(html: Data, title: String, in parent: NSWindow?) async throws -> URL? {
        let pdf = try await pdfData(html: html)
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.pdf]
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false
        panel.nameFieldStringValue = Self.suggestedFileName(title: title)
        let response = if let parent { await panel.beginSheetModal(for: parent) } else { panel.runModal() }
        guard response == .OK, let url = panel.url else { return nil }
        try pdf.write(to: url, options: .atomic)
        return url
    }

    /// The report printed to PDF. Throws when the page fails to load or nothing was printed.
    func pdfData(html: Data) async throws -> Data {
        let page = String(decoding: html, as: UTF8.self)
        let webView = makeWebView()
        defer { tearDown() }

        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            loaded = continuation
            webView.loadHTMLString(page, baseURL: nil)
        }

        guard let host = window else { throw ExportError.print }
        let file = URL.temporaryDirectory.appending(path: "tellfold-report-\(UUID().uuidString).pdf")
        defer { try? FileManager.default.removeItem(at: file) }
        let operation = webView.printOperation(with: printInfo(savingTo: file))
        operation.showsPrintPanel = false
        operation.showsProgressPanel = false
        operation.view?.frame = webView.bounds
        // The web view prints properly only from a window, so it runs modally for an offscreen one (a plain `run()` kept
        // writing pages without end).
        let succeeded = await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            let done = Unmanaged.passRetained(PrintCompletion(continuation)).toOpaque()
            operation.runModal(for: host, delegate: self,
                               didRun: #selector(printOperationDidRun(_:success:contextInfo:)), contextInfo: done)
        }
        guard succeeded, let data = try? Data(contentsOf: file), !data.isEmpty else { throw ExportError.print }
        return data
    }

    /// The report title as a file name: no path separators or colons, at most 80 characters, ending in `.pdf`.
    nonisolated static func suggestedFileName(title: String) -> String {
        let cleaned = title
            .components(separatedBy: CharacterSet(charactersIn: "/:\\").union(.newlines).union(.controlCharacters))
            .joined(separator: " ")
            .split(whereSeparator: \.isWhitespace)
            .joined(separator: " ")
            .trimmingCharacters(in: CharacterSet(charactersIn: ". "))
        let name = cleaned.isEmpty ? "Tellfold report" : String(cleaned.prefix(80)).trimmingCharacters(in: .whitespaces)
        return name + ".pdf"
    }

    // MARK: Plumbing

    private func makeWebView() -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        // Light on screen as well as on paper, whatever the system appearance.
        configuration.userContentController.addUserScript(WKUserScript(
            source: "document.documentElement.dataset.theme = 'light';",
            injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let webView = WKWebView(frame: Self.viewport, configuration: configuration)
        webView.appearance = NSAppearance(named: .aqua)
        webView.navigationDelegate = self
        let window = NSWindow(contentRect: Self.viewport.offsetBy(dx: -20_000, dy: -20_000), styleMask: [.borderless],
                              backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.appearance = NSAppearance(named: .aqua)
        window.contentView = webView
        self.webView = webView
        self.window = window
        return webView
    }

    private func printInfo(savingTo file: URL) -> NSPrintInfo {
        let info = NSPrintInfo(dictionary: [.jobSavingURL: file])
        info.jobDisposition = .save
        info.paperSize = paperSize
        info.orientation = .portrait
        info.horizontalPagination = .fit
        info.verticalPagination = .automatic
        info.isHorizontallyCentered = false
        info.isVerticallyCentered = false
        return info
    }

    private func tearDown() {
        webView?.navigationDelegate = nil
        window?.contentView = nil
        window?.close()
        webView = nil
        window = nil
    }

    private func finishLoading(failure: String? = nil) {
        guard let loaded else { return }
        self.loaded = nil
        if let failure { loaded.resume(throwing: ExportError.load(failure)) } else { loaded.resume() }
    }

    /// AppKit calls this on the print operation's own thread, so the continuation travels in `contextInfo`.
    @objc nonisolated private func printOperationDidRun(_ operation: NSPrintOperation, success: Bool,
                                                        contextInfo: UnsafeMutableRawPointer?) {
        guard let contextInfo else { return }
        Unmanaged<PrintCompletion>.fromOpaque(contextInfo).takeRetainedValue().continuation.resume(returning: success)
    }
}

private final class PrintCompletion: Sendable {
    let continuation: CheckedContinuation<Bool, Never>
    init(_ continuation: CheckedContinuation<Bool, Never>) { self.continuation = continuation }
}

extension ReportPDFExporter: WKNavigationDelegate {
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        finishLoading()
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        finishLoading(failure: error.localizedDescription)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        finishLoading(failure: error.localizedDescription)
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        finishLoading(failure: "the web content process stopped")
    }
}
