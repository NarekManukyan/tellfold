import Foundation
import UniformTypeIdentifiers

/// `.tellfold` run bundles: one finished run as a single file (a zip the engine writes and checks), declared in
/// `project.yml` as `dev.tellfold.run-bundle`. Opening one makes a read-only copy of someone else's run.
enum RunBundle {
    static let fileExtension = "tellfold"
    /// The media type the engine sends and the app declares (`Info.plist`).
    static let mediaType = "application/vnd.tellfold.run+zip"
    /// The engine refuses anything larger (`MAX_BUNDLE_BYTES` in `tellfold-study`).
    static let maxBytes = 512 * 1024 * 1024

    /// A `.tellfold` file on this Mac that cannot be read or written.
    struct FileError: Error, LocalizedError, Hashable, Sendable {
        var message: String
        var errorDescription: String? { message }
    }

    /// What the save panel says about the file.
    static let panelMessage = "The file holds the whole run: settings, documents, people, posts and report. Anyone with Tellfold can open it."

    /// Added for a run built from audience material: its people carry the quotes they were written from.
    static let groundedNote = "This run was built from your audience material: the file includes the cleaned quotes its people were written from."

    /// The save panel's message; a grounded run adds a line about its quotes.
    static func panelMessage(grounded: Bool) -> String { grounded ? panelMessage + "\n\n" + groundedNote : panelMessage }

    nonisolated static func isBundle(_ url: URL) -> Bool { url.pathExtension.lowercased() == fileExtension }

    /// The report title as a bundle file name, cleaned like the PDF's.
    static func suggestedFileName(title: String) -> String {
        let name = ReportPDFExporter.baseName(title: title)
        return (name == ReportPDFExporter.fallbackName ? "Tellfold run" : name) + "." + fileExtension
    }

    /// Throws when the file at `url` cannot be sent to the engine: unreadable, or larger than the engine accepts.
    static func checkReadable(_ url: URL) throws {
        let values: URLResourceValues
        do {
            values = try url.resourceValues(forKeys: [.fileSizeKey, .isReadableKey])
        } catch {
            throw FileError(message: "Could not read \(url.lastPathComponent): \(error.localizedDescription)")
        }
        if values.isReadable == false {
            throw FileError(message: "Could not read \(url.lastPathComponent).")
        }
        if (values.fileSize ?? 0) > maxBytes {
            throw FileError(message: "\(url.lastPathComponent) is larger than 512 MB.")
        }
    }
}

extension UTType {
    /// A Tellfold run bundle (`.tellfold`).
    static let tellfoldBundle = UTType(exportedAs: "dev.tellfold.run-bundle", conformingTo: .zip)
}
