import Foundation
import UniformTypeIdentifiers

/// `.tellfold` run bundles: one finished run as a single file (a zip the engine writes and checks), declared in
/// `project.yml` as `dev.tellfold.run-bundle`. Opening one makes a read-only copy of someone else's run.
enum RunBundle {
    static let fileExtension = "tellfold"
    /// The engine refuses anything larger (`MAX_BUNDLE_BYTES` in `tellfold-study`).
    static let maxBytes = 512 * 1024 * 1024

    nonisolated static func isBundle(_ url: URL) -> Bool { url.pathExtension.lowercased() == fileExtension }

    /// The report title as a bundle file name, cleaned like the PDF's.
    static func suggestedFileName(title: String) -> String {
        let pdf = ReportPDFExporter.suggestedFileName(title: title)
        let name = pdf.hasSuffix(".pdf") ? String(pdf.dropLast(4)) : pdf
        return (name == "Tellfold report" ? "Tellfold run" : name) + "." + fileExtension
    }

    /// The file's bytes, or why it cannot be sent to the engine.
    static func read(_ url: URL) throws -> Data {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        if size > maxBytes {
            throw TellfoldAPIError(status: 413, message: "\(url.lastPathComponent) is larger than 512 MB.")
        }
        do {
            return try Data(contentsOf: url, options: .mappedIfSafe)
        } catch {
            throw TellfoldAPIError(status: 0, message: "Could not read \(url.lastPathComponent): \(error.localizedDescription)")
        }
    }
}

extension UTType {
    /// A Tellfold run bundle (`.tellfold`).
    static let tellfoldBundle = UTType(exportedAs: "dev.tellfold.run-bundle", conformingTo: .zip)
}
