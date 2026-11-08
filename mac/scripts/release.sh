#!/bin/bash
# Builds a Developer ID signed, notarized Tellfold.dmg and adds it to the Sparkle appcast in site/appcast.xml.
#
#   mac/scripts/release.sh <version>             build, notarize and write the appcast entry
#   mac/scripts/release.sh <version> --publish   also create the GitHub release and push the appcast, which GitHub Pages serves
#
# Needs, once per Mac (see README "Releasing"):
#   - a "Developer ID Application" certificate in the login keychain, or TELLFOLD_TEAM_ID and an Xcode account
#     for that team that can use Apple's cloud-managed Developer ID certificate
#   - notary credentials: an App Store Connect API key saved as an `asc` profile (set TELLFOLD_ASC_PROFILE),
#     or `xcrun notarytool store-credentials tellfold-notary ...`
#   - the Sparkle signing key in the keychain (`generate_keys --account tellfold`)
#   - release notes in docs/releases/<version>.md
# Optional: TELLFOLD_SIGN_IDENTITY (certificate name or SHA-1), TELLFOLD_ASC_PROFILE (notarize with `asc`),
# TELLFOLD_NOTARY_PROFILE (notarytool keychain profile, default tellfold-notary, used when TELLFOLD_ASC_PROFILE is unset).
set -euo pipefail

VERSION=${1:?usage: release.sh <version> [--publish]}
PUBLISH=${2:-}
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: version must look like 1.2.3" >&2; exit 1; }
[[ -z "$PUBLISH" || "$PUBLISH" == "--publish" ]] || { echo "error: unknown option $PUBLISH" >&2; exit 1; }

MAC=$(cd "$(dirname "$0")/.." && pwd)
ROOT=$(dirname "$MAC")
REPO=NarekManukyan/tellfold
SITE_URL=https://narekmanukyan.github.io/tellfold
SPARKLE_VERSION=2.10.1
SPARKLE="$MAC/build/sparkle-$SPARKLE_VERSION"
NOTARY_PROFILE=${TELLFOLD_NOTARY_PROFILE:-tellfold-notary}
ASC_PROFILE=${TELLFOLD_ASC_PROFILE:-}
NOTES="$ROOT/docs/releases/$VERSION.md"
OUT="$MAC/build/release/$VERSION"
APPCAST="$ROOT/site/appcast.xml"

step() { printf '\n==> %s\n' "$*"; }
fail() { echo "error: $*" >&2; exit 1; }

# --- preflight -----------------------------------------------------------------------------------------------------

step "Checking the release can be built"
[ -z "$(git -C "$ROOT" status --porcelain)" ] || fail "commit or stash your changes first; a release is built from a commit"
[ -f "$NOTES" ] || fail "write the release notes first: $NOTES"
grep -q '<language>en</language>' "$APPCAST" || fail "$APPCAST is missing its <language>en</language> line"
grep -q "<sparkle:shortVersionString>$VERSION<" "$APPCAST" && fail "$VERSION is already in the appcast"

# Local mode signs with a Developer ID certificate in the keychain. Cloud mode lets Xcode sign the export with the
# team's cloud-managed Developer ID certificate; the disk image is then notarized but carries no signature of its own.
IDENTITY=${TELLFOLD_SIGN_IDENTITY:-}
if [ -z "$IDENTITY" ]; then
    IDENTITIES=$(security find-identity -v -p codesigning | grep '"Developer ID Application:' || true)
    if [ -n "$IDENTITIES" ]; then
        [ "$(printf '%s\n' "$IDENTITIES" | wc -l | tr -d ' ')" = 1 ] || fail "several Developer ID certificates; set TELLFOLD_SIGN_IDENTITY to one of them"
        IDENTITY=$(printf '%s\n' "$IDENTITIES" | sed -E 's/.*"(.*)"/\1/')
    fi
fi
if [ -n "$IDENTITY" ]; then
    MODE=local
    TEAM_ID=$(printf '%s\n' "$IDENTITY" | sed -nE 's/.*\(([A-Z0-9]{10})\)$/\1/p')
    [ -n "$TEAM_ID" ] || TEAM_ID=${TELLFOLD_TEAM_ID:?set TELLFOLD_TEAM_ID when TELLFOLD_SIGN_IDENTITY is a hash}
    echo "Signing as: $IDENTITY"
else
    MODE=cloud
    TEAM_ID=${TELLFOLD_TEAM_ID:-}
    [ -n "$TEAM_ID" ] || fail "no Developer ID Application certificate in the keychain; add one, or set TELLFOLD_TEAM_ID to sign with the team's cloud-managed certificate"
    echo "Signing with the cloud-managed Developer ID certificate of team $TEAM_ID"
fi

if [ -n "$ASC_PROFILE" ]; then
    asc --profile "$ASC_PROFILE" notarization list >/dev/null 2>&1 \
        || fail "the asc profile $ASC_PROFILE cannot reach the notary service (asc auth status)"
else
    xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 \
        || fail "no notary credentials named $NOTARY_PROFILE (xcrun notarytool store-credentials $NOTARY_PROFILE --apple-id <id> --team-id $TEAM_ID)"
fi

if [ ! -x "$SPARKLE/bin/sign_update" ]; then
    step "Downloading Sparkle $SPARKLE_VERSION tools"
    mkdir -p "$SPARKLE"
    curl -fsSL -o "$SPARKLE/sparkle.tar.xz" \
        "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
    tar -xf "$SPARKLE/sparkle.tar.xz" -C "$SPARKLE"
fi

if [ "$PUBLISH" = "--publish" ]; then
    gh release view "v$VERSION" --repo "$REPO" >/dev/null 2>&1 && fail "GitHub release v$VERSION already exists"
    git -C "$ROOT" fetch -q origin
    [ "$(git -C "$ROOT" rev-parse HEAD)" = "$(git -C "$ROOT" rev-parse '@{u}' 2>/dev/null)" ] \
        || fail "push this commit first; the GitHub release points at it"
fi

# Sparkle compares CFBundleVersion: the commit count only ever grows on main.
BUILD=$(git -C "$ROOT" rev-list --count HEAD)
rm -rf "$OUT"
mkdir -p "$OUT"

# --- build -----------------------------------------------------------------------------------------------------------

step "Archiving Tellfold $VERSION ($BUILD)"
(cd "$MAC" && xcodegen generate --quiet)
if [ "$MODE" = local ]; then
    SIGNING=(CODE_SIGN_STYLE=Manual CODE_SIGN_IDENTITY="$IDENTITY")
else
    SIGNING=(CODE_SIGN_STYLE=Automatic CODE_SIGN_IDENTITY="Apple Development" -allowProvisioningUpdates)
fi
xcodebuild -project "$MAC/Tellfold.xcodeproj" -scheme Tellfold -configuration Release \
    -derivedDataPath "$MAC/build/release-dd" -archivePath "$OUT/Tellfold.xcarchive" archive \
    MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$BUILD" DEVELOPMENT_TEAM="$TEAM_ID" \
    "${SIGNING[@]}" OTHER_CODE_SIGN_FLAGS="--timestamp" -quiet

APP="$OUT/Tellfold.app"
if [ "$MODE" = cloud ]; then
    step "Exporting with the cloud-managed Developer ID certificate"
    cat > "$OUT/ExportOptions.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>developer-id</string>
  <key>signingStyle</key><string>automatic</string>
  <key>teamID</key><string>$TEAM_ID</string>
  <key>destination</key><string>export</string>
</dict></plist>
EOF
    xcodebuild -exportArchive -archivePath "$OUT/Tellfold.xcarchive" -exportPath "$OUT/export" \
        -exportOptionsPlist "$OUT/ExportOptions.plist" -allowProvisioningUpdates -quiet
    ditto "$OUT/export/Tellfold.app" "$APP"
    codesign --verify --deep --strict "$APP"
else
ditto "$OUT/Tellfold.xcarchive/Products/Applications/Tellfold.app" "$APP"

step "Signing Sparkle and the app for Developer ID"
# Xcode signs the framework on copy but not the helpers inside it; sign inside out, never with --deep.
SPARKLE_FW="$APP/Contents/Frameworks/Sparkle.framework"
sign() { codesign --force --sign "$IDENTITY" --options runtime --timestamp "$@"; }
sign "$SPARKLE_FW/Versions/B/XPCServices/Installer.xpc"
sign --preserve-metadata=entitlements "$SPARKLE_FW/Versions/B/XPCServices/Downloader.xpc"
sign "$SPARKLE_FW/Versions/B/Autoupdate"
sign "$SPARKLE_FW/Versions/B/Updater.app"
sign "$SPARKLE_FW"
sign --entitlements "$MAC/Tellfold/App/Tellfold.entitlements" "$APP"
codesign --verify --deep --strict "$APP"
fi

# Pulls the submission id and status out of either tool's JSON.
submission() {
    python3 -c 'import json,sys
d=json.load(sys.stdin); d=d.get("data",d)
a=d.get("attributes",d)
print(d.get("id",""), a.get("status",""))'
}

notarize() {
    local file=$1 result id status
    if [ -n "$ASC_PROFILE" ]; then
        result=$(asc --profile "$ASC_PROFILE" notarization submit --file "$file" --wait --timeout 1h --output json)
        read -r id status < <(printf '%s' "$result" | submission)
        if [ "$status" != "Accepted" ]; then
            asc --profile "$ASC_PROFILE" notarization log --id "$id" >&2 || true
            fail "notarization of $(basename "$file") ended as ${status:-unknown}"
        fi
        echo "Notarized $(basename "$file") ($id)"
        return
    fi
    result=$(xcrun notarytool submit "$file" --keychain-profile "$NOTARY_PROFILE" --wait --output-format json)
    id=$(printf '%s' "$result" | plutil -extract id raw -o - -)
    status=$(printf '%s' "$result" | plutil -extract status raw -o - -)
    if [ "$status" != "Accepted" ]; then
        xcrun notarytool log "$id" --keychain-profile "$NOTARY_PROFILE" >&2 || true
        fail "notarization of $(basename "$file") ended as $status"
    fi
    echo "Notarized $(basename "$file") ($id)"
}

step "Notarizing the app (a few minutes)"
ditto -c -k --keepParent "$APP" "$OUT/Tellfold.zip"
notarize "$OUT/Tellfold.zip"
xcrun stapler staple -q "$APP"
rm "$OUT/Tellfold.zip"

step "Making Tellfold.dmg"
STAGE="$OUT/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Tellfold.app"
ln -s /Applications "$STAGE/Applications"
DMG="$OUT/Tellfold.dmg"
hdiutil create -quiet -volname Tellfold -srcfolder "$STAGE" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"
rm -rf "$STAGE"
[ "$MODE" = local ] && codesign --sign "$IDENTITY" --timestamp "$DMG"

step "Notarizing the disk image"
notarize "$DMG"
xcrun stapler staple -q "$DMG"
xcrun stapler validate -q "$DMG"
[ "$MODE" = local ] && spctl --assess --type open --context context:primary-signature "$DMG"
spctl --assess --type execute "$APP"

# --- appcast ---------------------------------------------------------------------------------------------------------

step "Adding $VERSION to the appcast"
SIGNATURE=$("$SPARKLE/bin/sign_update" --account tellfold "$DMG")
mkdir -p "$ROOT/site/releases"
cp "$NOTES" "$ROOT/site/releases/$VERSION.md"
ITEM="$OUT/item.xml"
cat > "$ITEM" <<EOF
    <item>
      <title>Tellfold $VERSION</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$BUILD</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>26.0.0</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>$SITE_URL/releases/$VERSION.md</sparkle:releaseNotesLink>
      <enclosure url="https://github.com/$REPO/releases/download/v$VERSION/Tellfold.dmg" type="application/octet-stream" $SIGNATURE />
    </item>
EOF
# Newest first, right after the channel header.
awk -v item="$ITEM" '{ print } /<language>en<\/language>/ { while ((getline line < item) > 0) print line }' \
    "$APPCAST" > "$OUT/appcast.xml"
xmllint --noout "$OUT/appcast.xml"
cp "$OUT/appcast.xml" "$APPCAST"

SIZE=$(du -h "$DMG" | cut -f1)
step "Built $DMG ($SIZE)"

if [ "$PUBLISH" != "--publish" ]; then
    cat <<EOF

Not published. To publish this build:
  gh release create v$VERSION "$DMG" --repo $REPO --title "Tellfold $VERSION" --notes-file "$NOTES" --target $(git -C "$ROOT" rev-parse HEAD)
Then commit and push site/appcast.xml and site/releases/$VERSION.md; GitHub Pages publishes them.
EOF
    exit 0
fi

step "Publishing the GitHub release"
gh release create "v$VERSION" "$DMG" --repo "$REPO" --title "Tellfold $VERSION" --notes-file "$NOTES" \
    --target "$(git -C "$ROOT" rev-parse HEAD)"

step "Pushing the appcast (GitHub Pages deploys it)"
git -C "$ROOT" add site/appcast.xml "site/releases/$VERSION.md"
git -C "$ROOT" commit -q -m "release: Tellfold $VERSION appcast"
git -C "$ROOT" push -q origin HEAD

step "Done. Tellfold $VERSION is on GitHub; the appcast is live once the Pages workflow finishes."
