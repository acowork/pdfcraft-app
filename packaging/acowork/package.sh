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
  # The web build embeds its translations and its CJK interface faces from local checkouts. Both are
  # build inputs: without the catalogs the package has no translations at all, and its only CJK face
  # is the Japanese one, which lacks a quarter of the simplified hanzi it renders.
  : "${CRAFT_I18N_DIR:?WITH_WEB_UI needs CRAFT_I18N_DIR (a craft-i18n checkout): a package without the catalogs cannot change language}"
  : "${CRAFT_FONTS_DIR:?WITH_WEB_UI needs CRAFT_FONTS_DIR (a craft-fonts checkout): the web build's CJK faces come from it}"
  pinned="$(tr -d '[:space:]' < "$ROOT/craft-i18n.rev" 2>/dev/null || true)"
  actual="$(git -C "$CRAFT_I18N_DIR" rev-parse HEAD 2>/dev/null || true)"
  if [[ -n "$actual" && -n "$pinned" && "$actual" != "$pinned" ]]; then
    echo "warning: craft-i18n at ${actual:0:8}, this app pins ${pinned:0:8}; build from a worktree at the pinned rev for a reproducible package" >&2
  fi
  export CRAFT_I18N_DIR CRAFT_FONTS_DIR
  # The CJK interface faces, cut to the characters the catalogs actually render: ~0.8 MB instead of
  # the 13 MB Chinese face the web build drops, and the Korean face it never had.
  font_out="$ROOT/dist/fonts"
  python3 "$(dirname "$0")/subset-ui-font.py" --fonts-dir "$CRAFT_FONTS_DIR" --i18n-dir "$CRAFT_I18N_DIR" --out-dir "$font_out"
  PDFCRAFT_UI_HANS_SUBSET="$font_out/pdfcraft-ui-hans.ttf"
  PDFCRAFT_UI_KORE_SUBSET="$font_out/pdfcraft-ui-kore.ttf"
  export PDFCRAFT_UI_HANS_SUBSET PDFCRAFT_UI_KORE_SUBSET
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
