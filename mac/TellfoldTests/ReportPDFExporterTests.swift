import AppKit
import PDFKit
import Testing
@testable import Tellfold

// File > Export PDF… prints the report's HTML from an offscreen web view. A stub page with a forced page break must come
// back as a real, paged PDF on the requested paper, with its text intact.
@MainActor
@Suite("Report PDF export")
struct ReportPDFExporterTests {
    private static let stub = """
        <!doctype html><html><head><meta charset="utf-8"><title>Stub</title>
        <style>@media print { .next { break-before: page; } }</style></head>
        <body><h1>Free tier keeps light users</h1><p>Cited post #12 by @carol_and_duke.</p>
        <section class="next"><h2>Persona directory</h2><p>Second page.</p></section></body></html>
        """

    @Test func stubHTMLBecomesAPagedPDF() async throws {
        let a4 = NSSize(width: 595, height: 842)
        let data = try await ReportPDFExporter(paperSize: a4).pdfData(html: Data(Self.stub.utf8))
        #expect(data.starts(with: Data("%PDF".utf8)))
        let pdf = try #require(PDFDocument(data: data))
        #expect(pdf.pageCount == 2)
        let page = try #require(pdf.page(at: 0))
        let bounds = page.bounds(for: .mediaBox)
        #expect(abs(bounds.width - a4.width) < 1 && abs(bounds.height - a4.height) < 1)
        #expect(pdf.string?.contains("Free tier keeps light users") == true)
        #expect(pdf.page(at: 1)?.string?.contains("Persona directory") == true)
    }

    @Test func letterPaperIsHonoured() async throws {
        let letter = NSSize(width: 612, height: 792)
        let data = try await ReportPDFExporter(paperSize: letter).pdfData(html: Data(Self.stub.utf8))
        let bounds = try #require(PDFDocument(data: data)?.page(at: 0)).bounds(for: .mediaBox)
        #expect(abs(bounds.width - letter.width) < 1 && abs(bounds.height - letter.height) < 1)
    }

    @Test(arguments: [
        ("Free tier keeps light users", "Free tier keeps light users.pdf"),
        ("Plus: yes/no?\nMaybe", "Plus yes no? Maybe.pdf"),
        ("  ...  ", "Tellfold report.pdf"),
        ("", "Tellfold report.pdf"),
    ])
    func suggestedFileNameIsSafe(title: String, expected: String) {
        #expect(ReportPDFExporter.suggestedFileName(title: title) == expected)
    }

    @Test func suggestedFileNameIsCapped() {
        let name = ReportPDFExporter.suggestedFileName(title: String(repeating: "word ", count: 40))
        #expect(name.hasSuffix(".pdf") && name.count <= 84)
        #expect(!name.contains(" .pdf"))
    }
}
