#!/bin/sh
# Downloads the music and sound effects the main cut uses into public/audio/. They are Mixkit files under the Mixkit
# Free License (music: Stock Music Free License; effects: Sound Effects Free License), which allows them in videos
# but not passing the files on by themselves, so they are not committed.
set -eu
cd "$(dirname "$0")/public"
mkdir -p audio
get() { [ -s "audio/$1" ] || curl -fsSL "$2" -o "audio/$1"; }
get house-vibez.mp3 https://assets.mixkit.co/music/745/745.mp3
get paper-slide.mp3 https://assets.mixkit.co/active_storage/sfx/1530/1530-preview.mp3
get paper-page-turn.mp3 https://assets.mixkit.co/active_storage/sfx/1104/1104-preview.mp3
get switch-tap.mp3 https://assets.mixkit.co/active_storage/sfx/2585/2585-preview.mp3
get switch-click-quick.mp3 https://assets.mixkit.co/active_storage/sfx/2582/2582-preview.mp3
get impact-deep-whoosh.mp3 https://assets.mixkit.co/active_storage/sfx/1143/1143-preview.mp3
get whoosh-swirl.mp3 https://assets.mixkit.co/active_storage/sfx/1493/1493-preview.mp3
get stardust-swish.mp3 https://assets.mixkit.co/active_storage/sfx/1498/1498-preview.mp3
get air-woosh-deep.mp3 https://assets.mixkit.co/active_storage/sfx/2604/2604-preview.mp3
get shimmer-sparkle-sweep.mp3 https://assets.mixkit.co/active_storage/sfx/2633/2633-preview.mp3
echo "audio ready in public/audio"
