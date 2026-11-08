#!/bin/sh
# Makes the small, silent files the site and README use from the loop masters that `node render.mjs loop loop-11`
# renders.
#   site/assets/video/tellfold-loop.webm, .mp4   1280x720, under ~3 MB, for the landing page
#   site/assets/video/tellfold-loop-square.*     720x720, the same loop for phones, with its own poster
#   site/assets/video/tellfold-loop-poster.webp  the still shown before playback and to people who ask for less motion
#   out/tellfold-preview.webp                    animated WebP for the README, under ~8 MB
set -eu
cd "$(dirname "$0")"
src=out/tellfold-loop-master.mp4
site=../../site/assets/video
mkdir -p "$site"
tmp=$(mktemp -d)
trap 'rm -r "$tmp"' EXIT

ffmpeg -loglevel error -y -i "$src" -an -vf scale=1280:720:flags=lanczos -c:v libx264 -preset slow -crf 24 \
	-profile:v high -pix_fmt yuv420p -movflags +faststart "$site/tellfold-loop.mp4"
ffmpeg -loglevel error -y -i "$src" -an -vf scale=1280:720:flags=lanczos -c:v libvpx-vp9 -crf 34 -b:v 0 \
	-row-mt 1 -deadline good -cpu-used 2 "$site/tellfold-loop.webm"

# Poster: the café crowd at round 3, posts in the feed and the mood line at +0.70.
ffmpeg -loglevel error -y -ss 10.2 -i "$src" -frames:v 1 -vf scale=1280:720:flags=lanczos "$tmp/poster.png"
cwebp -quiet -q 82 "$tmp/poster.png" -o "$site/tellfold-loop-poster.webp"

# The square loop for phones, and its poster from the same moment.
sq=out/tellfold-loop-square-master.mp4
ffmpeg -loglevel error -y -i "$sq" -an -vf scale=720:720:flags=lanczos -c:v libx264 -preset slow -crf 24 \
	-profile:v high -pix_fmt yuv420p -movflags +faststart "$site/tellfold-loop-square.mp4"
ffmpeg -loglevel error -y -i "$sq" -an -vf scale=720:720:flags=lanczos -c:v libvpx-vp9 -crf 34 -b:v 0 \
	-row-mt 1 -deadline good -cpu-used 2 "$site/tellfold-loop-square.webm"
ffmpeg -loglevel error -y -ss 10.2 -i "$sq" -frames:v 1 -vf scale=720:720:flags=lanczos "$tmp/poster-square.png"
cwebp -quiet -q 82 "$tmp/poster-square.png" -o "$site/tellfold-loop-square-poster.webp"

# README: 960 wide, 12 frames a second, lossy animated WebP.
ffmpeg -loglevel error -y -i "$src" -vf fps=12,scale=960:540:flags=lanczos "$tmp/f%04d.png"
img2webp -loop 0 -lossy -q 78 -m 6 -d 83 "$tmp"/f*.png -o out/tellfold-preview.webp >/dev/null

ls -l "$site"/tellfold-loop* out/tellfold-preview.webp
