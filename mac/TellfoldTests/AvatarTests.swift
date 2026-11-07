import Foundation
import SwiftUI
import Testing
@testable import Tellfold

/// The avatar is a port of `tellfold/web/static/js/avatar.js`: same persona, same face on the web and in the Mac app.
/// The vectors below come from that reference implementation.
@Suite("Avatars")
struct AvatarTests {
    struct Vector: Sendable {
        let id: String, handle: String
        let seed: UInt32
        let back: String, skin: String, hair: String, shirt: String
        let style: Int
        let glasses: Bool
        let smile: Int
        let tile: String
    }

    static let vectors: [Vector] = [
        Vector(id: "a01", handle: "appstore", seed: 3_442_136_939, back: "#E0E7FF", skin: "#F6D3B8", hair: "#DDB874",
               shirt: "#8B5CF6", style: 6, glasses: false, smile: 1, tile: "#B45309"),
        Vector(id: "a07", handle: "rina_runsclub", seed: 1_292_670_260, back: "#E0F2FE", skin: "#5F3C28", hair: "#7A4E2D",
               shirt: "#6366F1", style: 6, glasses: true, smile: 0, tile: "#BE185D"),
        Vector(id: "a12", handle: "firsttime_tessa", seed: 2_613_731_273, back: "#E0E7FF", skin: "#B98059", hair: "#B5813E",
               shirt: "#6366F1", style: 2, glasses: true, smile: 2, tile: "#0369A1"),
        Vector(id: "x", handle: "", seed: 392_370_567, back: "#F1F5F9", skin: "#5F3C28", hair: "#DDB874",
               shirt: "#F59E0B", style: 4, glasses: true, smile: 1, tile: "#15803D"),
    ]

    @Test(arguments: vectors)
    func matchesTheReferenceImplementation(v: Vector) {
        #expect(Avatar.seed(id: v.id, handle: v.handle) == v.seed)
        let spec = Avatar.spec(id: v.id, handle: v.handle)
        #expect(spec == AvatarSpec(back: v.back, skin: v.skin, hair: v.hair, shirt: v.shirt, style: v.style,
                                   glasses: v.glasses, smile: v.smile))
        #expect(Avatar.tileColor(id: v.id, handle: v.handle) == v.tile)
    }

    @Test func initialsFollowTheReferenceRules() {
        #expect(Avatar.initials("App Store", "appstore") == "AS")
        #expect(Avatar.initials("revenuecat") == "RE")
        #expect(Avatar.initials("", "rina_runsclub") == "RR")  // the handle's underscores split words
        #expect(Avatar.initials("@x") == "X")
        #expect(Avatar.initials("") == "?")
    }

    @Test func samePersonaSameFaceDifferentPersonaDifferentSeed() {
        #expect(Avatar.spec(id: "a01", handle: "appstore") == Avatar.spec(id: "a01", handle: "appstore"))
        #expect(Avatar.seed(id: "a01", handle: "appstore") != Avatar.seed(id: "a01", handle: "appstore2"))
        #expect(Avatar.seed(id: "", handle: "") != 0)
    }

    @Test func everyChoiceStaysInRange() {
        for i in 0..<500 {
            let spec = Avatar.spec(id: "a\(i)", handle: "user_\(i * 7)")
            #expect(Avatar.back.contains(spec.back) && Avatar.skin.contains(spec.skin))
            #expect(Avatar.hair.contains(spec.hair) && Avatar.shirt.contains(spec.shirt))
            #expect((0..<8).contains(spec.style) && (0..<3).contains(spec.smile))
        }
    }

    @MainActor
    @Test func cacheRendersEachIdentityOnce() throws {
        let cache = AvatarImageCache()
        let person = AvatarIdentity(id: "a07", handle: "rina_runsclub", name: "Rina Alvarez", isOrganisation: false)
        let org = AvatarIdentity(id: "a01", handle: "appstore", name: "App Store", isOrganisation: true)
        let a = try #require(cache.image(for: person))
        let again = try #require(cache.image(for: person))
        #expect(a === again)
        #expect(cache.image(for: org) != nil)
        #expect(cache.count == 2)
        #expect(a.width == Int(AvatarImageCache.pixels) && a.height == Int(AvatarImageCache.pixels))
        #expect(cache.cached(for: AvatarIdentity(id: "zz", handle: "zz", name: "ZZ", isOrganisation: false)) == nil)
    }
}
