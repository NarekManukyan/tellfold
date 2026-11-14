#!/bin/bash
# Packs a built Tellfold.app into the drag-to-install disk image: a white window with the app on top and the
# Applications folder below it, inside a tinted drop zone with an arrow pointing into it.
#
#   mac/scripts/make-dmg.sh <Tellfold.app> <output.dmg>
#
# release.sh calls this, then signs and notarizes the image. Run it on its own to try a layout change without a
# release. It needs a logged-in session: Finder lays out the window (through AppleScript) and saves the layout in the
# volume's .DS_Store, the same way create-dmg does. The first run asks to let the terminal control Finder.
#
# The background is mac/dmg/background.png and background@2x.png, drawn from mac/dmg/background.svg. If you change
# the picture, keep the icon centres below in step with it.
set -euo pipefail

APP=${1:?usage: make-dmg.sh <Tellfold.app> <output.dmg>}
DMG=${2:?usage: make-dmg.sh <Tellfold.app> <output.dmg>}
HERE=$(cd "$(dirname "$0")/.." && pwd)
VOLNAME=Tellfold

# Window content size and icon centres, in points. They match mac/dmg/background.svg.
WIN_W=340
WIN_H=420
APP_POS="170, 100"
APPS_POS="170, 296"

fail() { echo "error: $*" >&2; exit 1; }
[ -d "$APP" ] || fail "no app at $APP"
[ -e "/Volumes/$VOLNAME" ] && fail "/Volumes/$VOLNAME is already mounted; eject it first"

WORK=$(mktemp -d)
MOUNT=""
cleanup() {
    [ -n "$MOUNT" ] && hdiutil detach -quiet -force "$MOUNT" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# The volume's contents. HFS+, not APFS: Finder keeps window backgrounds reliably only on HFS+.
STAGE="$WORK/stage"
mkdir -p "$STAGE/.background"
ditto "$APP" "$STAGE/Tellfold.app"
ln -s /Applications "$STAGE/Applications"
# One TIFF with both resolutions, so the background is sharp on Retina screens.
tiffutil -cathidpicheck "$HERE/dmg/background.png" "$HERE/dmg/background@2x.png" -out "$STAGE/.background/background.tiff" 2>/dev/null

RW="$WORK/rw.dmg"
hdiutil create -quiet -volname "$VOLNAME" -srcfolder "$STAGE" -fs HFS+ -fsargs "-c c=64,a=16,e=16" -format UDRW \
    -size "$(( $(du -sm "$STAGE" | cut -f1) + 20 ))m" "$RW"
MOUNT=$(hdiutil attach -readwrite -noverify -noautoopen "$RW" | awk -F'\t' '/Apple_HFS/ {print $NF}')
[ "$MOUNT" = "/Volumes/$VOLNAME" ] || fail "the image mounted at $MOUNT, not /Volumes/$VOLNAME"
# Finder bounds include the title bar, so the window is that much taller than the picture.
TITLE_H=32
sleep 2
osascript <<EOF
tell application "Finder"
    tell disk "$VOLNAME"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {200, 120, 200 + $WIN_W, 120 + $WIN_H + $TITLE_H}
        set opts to icon view options of container window
        set arrangement of opts to not arranged
        set icon size of opts to 128
        set text size of opts to 13
        set shows item info of opts to false
        set shows icon preview of opts to true
        set position of item "Tellfold.app" of container window to {$APP_POS}
        set position of item "Applications" of container window to {$APPS_POS}
        -- Set the picture last: changing other view options afterwards can drop it.
        set background picture of opts to file ".background:background.tiff"
        close
        open
        update without registering applications
        delay 2
        close
    end tell
end tell
EOF

# Wait for Finder to write the layout, then tidy up and seal the image.
for _ in 1 2 3 4 5 6 7 8 9 10; do [ -f "$MOUNT/.DS_Store" ] && break; sleep 1; done
[ -f "$MOUNT/.DS_Store" ] || fail "Finder did not save the window layout"
# The disk icon on the desktop and in the window title is the app icon. Added after the layout, because Finder
# deletes it while it arranges the window.
cp "$APP/Contents/Resources/AppIcon.icns" "$MOUNT/.VolumeIcon.icns"
SetFile -c icnC "$MOUNT/.VolumeIcon.icns"
SetFile -a C "$MOUNT"
chmod -Rf go-w "$MOUNT" 2>/dev/null || true
rm -rf "$MOUNT/.fseventsd" "$MOUNT/.Trashes"
sync
hdiutil detach -quiet "$MOUNT" || { sleep 3; hdiutil detach -quiet -force "$MOUNT"; }
MOUNT=""

rm -f "$DMG"
hdiutil convert -quiet "$RW" -format UDZO -imagekey zlib-level=9 -o "$DMG"
echo "Made $DMG ($(du -h "$DMG" | cut -f1))"
