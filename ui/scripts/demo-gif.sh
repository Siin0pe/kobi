#!/usr/bin/env bash
# GIF de démonstration du README (KOB-112) : rejoue la scène de
# tests/demo-gif.spec.ts dans Chromium, puis en fait un GIF léger
# (bande utile de la scène, 10 images/s, 480 px, 32 couleurs, palette calculée sur la vidéo).
set -euo pipefail
cd "$(dirname "$0")/.."
rm -rf test-results/demo-gif-*
KOBI_DEMO=1 npx playwright test tests/demo-gif.spec.ts --reporter=line
video=$(ls test-results/demo-gif-*/video.webm)
sortie=../docs/demo/kobi-demo.gif
mkdir -p "$(dirname "$sortie")"
filtre="crop=820:380:0:60,fps=10,scale=480:-1:flags=lanczos"
ffmpeg -loglevel error -y -i "$video" -vf "$filtre,palettegen=max_colors=32:stats_mode=diff" /tmp/kobi-palette.png
ffmpeg -loglevel error -y -i "$video" -i /tmp/kobi-palette.png -lavfi "$filtre [x]; [x][1:v] paletteuse=dither=none:diff_mode=rectangle" "$sortie"
rm -f /tmp/kobi-palette.png
ls -l "$sortie"
