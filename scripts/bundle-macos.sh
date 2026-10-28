#!/usr/bin/env bash
# Builds Forge.app from the release binary.
#
#   scripts/bundle-macos.sh             build release + bundle into dist/Forge.app
#   scripts/bundle-macos.sh --install   also install to /Applications and link `forge` CLI
#   scripts/bundle-macos.sh --no-build  bundle the existing target/release/forge
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BUILD=1
INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    --install) INSTALL=1 ;;
    *) echo "unknown option: $arg" >&2; exit 1 ;;
  esac
done

APP_NAME="Forge"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
BIN="$ROOT/target/release/forge"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BUILD_NUMBER="$(git rev-list --count HEAD 2>/dev/null || echo 1)"

if [[ "$BUILD" == 1 ]]; then
  echo "==> cargo build --release"
  cargo build --release
fi
[[ -x "$BIN" ]] || { echo "missing $BIN (run without --no-build)" >&2; exit 1; }

echo "==> Bundling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/forge"
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD_NUMBER/" macos/Info.plist > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# Icon: rendered once, cached in dist/.
ICNS="$DIST/Forge.icns"
if [[ ! -f "$ICNS" || macos/make-icon.swift -nt "$ICNS" ]]; then
  echo "==> Rendering icon"
  WORK="$(mktemp -d)"
  swift macos/make-icon.swift "$WORK/icon.png"
  ICONSET="$WORK/Forge.iconset"
  mkdir -p "$ICONSET"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$WORK/icon.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    sips -z $((s * 2)) $((s * 2)) "$WORK/icon.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$ICNS"
  rm -rf "$WORK"
fi
cp "$ICNS" "$APP/Contents/Resources/Forge.icns"

echo "==> Ad-hoc signing"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

if [[ "$INSTALL" == 1 ]]; then
  DEST="/Applications/$APP_NAME.app"
  echo "==> Installing to $DEST"
  osascript -e 'quit app "Forge"' >/dev/null 2>&1 || true
  pkill -x forge >/dev/null 2>&1 || true
  rm -rf "$DEST"
  ditto "$APP" "$DEST"
  # Refresh Launch Services so "Open With" and the Dock pick up the new bundle.
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST" || true

  # `forge .` from a terminal.
  CLI_DIR="$HOME/.local/bin"
  mkdir -p "$CLI_DIR"
  cat > "$CLI_DIR/forge" <<'EOF'
#!/usr/bin/env bash
# Opens files/folders in Forge.app (absolute paths, so `forge .` works).
args=()
for p in "$@"; do args+=("$(cd "$(dirname "$p")" 2>/dev/null && pwd)/$(basename "$p")"); done
if [[ ${#args[@]} -eq 0 ]]; then exec open -a Forge; fi
exec open -a Forge "${args[@]}"
EOF
  chmod +x "$CLI_DIR/forge"
  echo "==> CLI: $CLI_DIR/forge"
  case ":$PATH:" in
    *":$CLI_DIR:"*) ;;
    *) echo "    add to PATH: echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc" ;;
  esac
fi

du -sh "$APP" | awk '{print "==> Done: " $2 " (" $1 ")"}'
