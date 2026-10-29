#!/usr/bin/env bash
# Shrinks every SVG icon in place with svgo (pinned). Run after
# scripts/gen_file_icons.py. Keeps viewBox and the 32x32 size attributes.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
before=$(cat "$ROOT"/assets/file-icons/*.svg "$ROOT"/assets/icons/*.svg | wc -c)
npx --yes svgo@3.3.2 --config "$ROOT/scripts/svgo/svgo.config.mjs" -q -f "$ROOT/assets/file-icons"
npx --yes svgo@3.3.2 --config "$ROOT/scripts/svgo/svgo.config.mjs" -q -f "$ROOT/assets/icons"
after=$(cat "$ROOT"/assets/file-icons/*.svg "$ROOT"/assets/icons/*.svg | wc -c)
echo "icons: $before -> $after bytes"
