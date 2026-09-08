#!/usr/bin/env bash
# Rebuild assets/startup/glasshouse.frames from the three.js scene beside it.
#
# Needs ~/projects/anythingToSomethingGreat (a2sg) built once: `npm install &&
# npm run build`. a2sg renders the scene headlessly in Chromium and abstracts
# each frame into glyphs; pack.py then strips its web player away and keeps the
# text. Nothing in the Glasshouse build depends on this script - it is how the
# checked-in asset was made, and how to change it.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
A2SG="${A2SG_DIR:-$HOME/projects/anythingToSomethingGreat}"
WORK="${TMPDIR:-/tmp}/glasshouse-startup"
mkdir -p "$WORK"

# 634x320 with asciiCell=16 lands exactly on 68x20 glyphs: 634/(16*28/48) = 68,
# 320/16 = 20. The scene frames itself to 68/2 x 20 to cancel the terminal's
# taller-than-wide cell.
#
# customChars is nine glyphs, none of them one of a2sg's four contour glyphs
# `-/|\`, which is what lets a tier mean one thing. Nine is also not a free
# choice: ramp-plus-contour must total fourteen for a2sg to lay the glyph atlas
# out four-by-four, and at other counts its contour lookup lands a row off and
# draws horizontals as `/`. Verified by rendering one frame per candidate.
node "$A2SG/dist-node/node/cli.js" render \
  -i "$HERE/glasshouse-scene.mjs" \
  -p terminal-ghost -t ascii-dom -o "$WORK/render.html" \
  -w 634 --height 320 --frames 96 --fps 12.5 \
  --set charset=custom --set 'customChars=.:;=+*#%@' \
  --set asciiColor=ink --set trails=0 --set scanlines=0 \
  --set grain=0 --set bloom=0.12 --set vignette=0.08 --set asciiCell=16 \
  --json > "$WORK/manifest.json"

python3 "$HERE/pack.py" "$WORK/render.html" "$HERE/glasshouse.frames" --interval=80
