#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PLATFORM="${PLATFORM:-}"
WITH_WEB_UI="${WITH_WEB_UI:-0}"
WEB_ARCHIVE="${WEB_ARCHIVE:-}"
OUT_DIR="${OUT_DIR:-}"

if [[ -z "$PLATFORM" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) PLATFORM=linux-x86_64 ;;
    Linux-aarch64) PLATFORM=linux-aarch64 ;;
    Darwin-arm64|Darwin-aarch64) PLATFORM=macos-aarch64 ;;
    Darwin-x86_64) PLATFORM=macos-x86_64 ;;
    *) echo "set PLATFORM=..."; exit 1 ;;
  esac
fi

if [[ -z "$OUT_DIR" ]]; then
  OUT_DIR="$ROOT/dist/acowork/pdfcraft-$PLATFORM"
fi

echo "Building pdfcraft-acowork (release)..."
(
  cd "$ROOT"
  cargo build -p pdfcraft-acowork --release
)
TARGET_DIR="$(cd "$ROOT" && cargo metadata --format-version 1 --no-deps | python -c "import json,sys; print(json.load(sys.stdin)['target_directory'])")"
BIN_SRC="$TARGET_DIR/release/pdfcraft-acowork"
[[ -f "$BIN_SRC" ]] || { echo "missing $BIN_SRC (target_directory=$TARGET_DIR)"; exit 1; }

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR/bin" "$OUT_DIR/ui" "$OUT_DIR/assets"
cp "$BIN_SRC" "$OUT_DIR/bin/pdfcraft-acowork"
chmod +x "$OUT_DIR/bin/pdfcraft-acowork"

ENTRY="bin/pdfcraft-acowork"
sed -e "s|platforms = \[[^]]*\]|platforms = [\"$PLATFORM\"]|" \
    -e "s|entry = \"[^\"]*\"|entry = \"$ENTRY\"|" \
    "$ROOT/packaging/acowork/app.toml" > "$OUT_DIR/app.toml"

if [[ -n "$WEB_ARCHIVE" && "$WITH_WEB_UI" == "1" ]]; then
  echo "pass either WITH_WEB_UI=1 or WEB_ARCHIVE, not both" >&2
  exit 1
fi
if [[ -n "$WEB_ARCHIVE" ]]; then
  [[ -f "$WEB_ARCHIVE" ]] || { echo "missing web archive: $WEB_ARCHIVE" >&2; exit 1; }
  echo "Unpacking prebuilt web UI from $WEB_ARCHIVE"
  tmp="$(mktemp -d)"
  unzip -q "$WEB_ARCHIVE" -d "$tmp"
  index="$(find "$tmp" -name index.html -print -quit)"
  [[ -n "$index" ]] || { echo "web archive has no index.html" >&2; exit 1; }
  cp -R "$(dirname "$index")/." "$OUT_DIR/ui/"
  rm -rf "$tmp"
elif [[ "$WITH_WEB_UI" == "1" ]]; then
  echo "Building pdfcraft-web via trunk..."
  (cd "$ROOT/apps/pdfcraft-web" && trunk build --release)
  cp -R "$ROOT/dist/web/." "$OUT_DIR/ui/"
else
  cp -R "$ROOT/packaging/acowork/ui/." "$OUT_DIR/ui/"
fi

if [[ -f "$ROOT/assets/app-icon/hicolor/512x512/apps/ai.storyteller.pdfcraft.png" ]]; then
  cp "$ROOT/assets/app-icon/hicolor/512x512/apps/ai.storyteller.pdfcraft.png" "$OUT_DIR/assets/icon.png"
elif [[ -f "$ROOT/assets/icon/icon_512.png" ]]; then
  cp "$ROOT/assets/icon/icon_512.png" "$OUT_DIR/assets/icon.png"
elif [[ -f "$ROOT/assets/app-icon/pdfcraft-512.png" ]]; then
  cp "$ROOT/assets/app-icon/pdfcraft-512.png" "$OUT_DIR/assets/icon.png"
elif [[ -f "$ROOT/assets/app-icon/pdfcraft-macos-512.png" ]]; then
  cp "$ROOT/assets/app-icon/pdfcraft-macos-512.png" "$OUT_DIR/assets/icon.png"
fi

cp "$ROOT/LICENSE-MIT" "$OUT_DIR/LICENSE-MIT"
cp "$ROOT/LICENSE-APACHE" "$OUT_DIR/LICENSE-APACHE"
cp "$ROOT/packaging/acowork/NOTICE" "$OUT_DIR/NOTICE"
if [[ -f "$ROOT/assets/app-icon/LICENSE.txt" ]]; then
  cp "$ROOT/assets/app-icon/LICENSE.txt" "$OUT_DIR/assets/ICON-LICENSE.txt"
fi

echo "Staged unsigned app tree at $OUT_DIR"
echo "Next: acowork-pack-app --app-dir \"$OUT_DIR\" --output \"$OUT_DIR.app\""
