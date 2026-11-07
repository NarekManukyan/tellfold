import Foundation
@testable import Shoal

private final class FixtureBundleToken {}

/// Loads files from the test bundle's `Fixtures` folder (captured from a real engine; see Fixtures/README.md).
enum Fixtures {
    static var bundle: Bundle { Bundle(for: FixtureBundleToken.self) }

    static func url(_ name: String) throws -> URL {
        let base = (name as NSString).deletingPathExtension
        let ext = (name as NSString).pathExtension
        if let url = bundle.url(forResource: base, withExtension: ext, subdirectory: "Fixtures") { return url }
        if let url = bundle.url(forResource: base, withExtension: ext) { return url }
        throw FixtureError.missing(name)
    }

    static func data(_ name: String) throws -> Data {
        try Data(contentsOf: url(name))
    }

    static func string(_ name: String) throws -> String {
        String(decoding: try data(name), as: UTF8.self)
    }

    static func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
        try ShoalAPI.decoder.decode(T.self, from: data(name))
    }

    enum FixtureError: Error { case missing(String) }
}
