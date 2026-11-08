#!/bin/sh
# Product Hunt gallery images: clean 1270x760 PNGs of key moments of the main cut, rendered from the source (not
# grabbed from the compressed video). 1920x1080 frames are scaled to 760 high and trimmed evenly at the sides,
# which leaves the captions and the feed column the same distance from each edge.
set -eu
cd "$(dirname "$0")"
dir=out/producthunt
mkdir -p "$dir"
tmp=$(mktemp -d)
trap 'rm -r "$tmp"' EXIT
shot() { # frame name
	./node_modules/.bin/remotion still src/index.ts Main-169 "$tmp/$2.png" --frame="$1" --log=error
	ffmpeg -loglevel error -y -i "$tmp/$2.png" -vf "scale=-2:760:flags=lanczos,crop=1270:760" "$dir/$2.png"
}
shot 118 1-news           # three notes someone is about to send
shot 567 2-crowd          # the café crowd in round 3, mood at +0.70
shot 770 3-breaking-news  # the school run: breaking news lands, mood falls to -0.13
shot 1080 4-report        # the Mossbook report, citations lit
shot 1205 5-app           # the real app
shot 1352 6-end           # the mark, the line, Free for Mac
ls -l "$dir"
