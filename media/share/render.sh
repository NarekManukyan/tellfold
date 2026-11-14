#!/bin/sh
# Renders card.html to the landing page's link preview, site/assets/img/share-1200x630.jpg (og:image, twitter:image).
# Uses the installed Google Chrome, headless, at 2x for sharp text, then scales down to 1200x630.
set -eu
cd "$(dirname "$0")"
chrome="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
tmp=$(mktemp -d)
trap 'rm -r "$tmp"' EXIT

"$chrome" --headless --disable-gpu --hide-scrollbars --force-device-scale-factor=2 --window-size=1200,630 \
	--allow-file-access-from-files --user-data-dir="$tmp/profile" \
	--screenshot="$tmp/card.png" "file://$PWD/card.html" 2>/dev/null
magick "$tmp/card.png" -resize 1200x630 -strip -quality 86 -sampling-factor 4:2:0 -interlace JPEG \
	../../site/assets/img/share-1200x630.jpg
ls -l ../../site/assets/img/share-1200x630.jpg
