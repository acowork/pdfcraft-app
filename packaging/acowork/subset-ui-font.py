#!/usr/bin/env python3
"""Build the CJK interface faces for a web (wasm) PdfCraft build.

Why this exists: the web build embeds one CJK face to stay within the hosting size budget, and the
one it keeps is the Japanese face (BIZ UDPGothic, ~5 MB). It has no simplified-only hanzi — `们`,
`这`, `识` and a quarter of this interface's own Chinese catalog (303 of 1046 characters) draw
nothing — and no Hangul at all. Native builds get the full LXGW WenKai Lite face plus installed
system fonts; a browser has neither, so a Chinese or Korean interface shows boxes however the
language was chosen.

The interface only ever draws the strings in the embedded catalogs, so subsetting the right faces
to exactly those codepoints is enough: 13.3 MB becomes about 0.6 MB, and Korean gains a face it
never had on the web (~0.2 MB). The Chinese face carries both the Simplified and the Traditional
catalog, because LXGW covers both and the host offers both (`zh-CN`, `zh-TW`).

The results are derived faces, so they carry their own names (`PdfCraft UI Hans`, `PdfCraft UI
Kore`): the OFL requires a modified version not to claim the upstream family name, and
`crates/fonts/build.rs` matches on that name when it embeds the file.

    python packaging/acowork/subset-ui-font.py \
        --fonts-dir /path/to/craft-fonts \
        --i18n-dir /path/to/craft-i18n \
        --out-dir dist/fonts

The build then embeds them with `PDFCRAFT_UI_HANS_SUBSET` / `PDFCRAFT_UI_KORE_SUBSET` (see
`crates/fonts/build.rs`). Nothing is downloaded: both inputs are local checkouts, as with
`CRAFT_FONTS_DIR` and `CRAFT_I18N_DIR`.
"""

import argparse
import os
import pathlib
import sys

# The characters the interface faces must carry, by script. Keeping the filter to these ranges
# means a stray Latin letter in a translation cannot pull a whole alphabet of CJK-face Latin
# glyphs into the file.
IDEOGRAPH_RANGES = (
    (0x2E80, 0x2EFF),  # CJK radicals supplement
    (0x3000, 0x303F),  # CJK symbols and punctuation (、。「」…)
    (0x3400, 0x4DBF),  # ext A
    (0x4E00, 0x9FFF),  # unified ideographs
    (0xF900, 0xFAFF),  # compatibility ideographs
    (0xFF00, 0xFFEF),  # full-width forms (（）：)
)
HANGUL_RANGES = (
    (0x1100, 0x11FF),  # Jamo
    (0x3130, 0x318F),  # Compatibility Jamo
    (0xAC00, 0xD7A3),  # syllables
)

# Two faces, one per script the web build cannot otherwise show:
#   LXGW WenKai Lite — the face PdfCraft's native build uses for Chinese, so the hosted build
#     ships the same letterforms. Cut to the Simplified and Traditional catalogs (~0.6 MB).
#   NanumGothic — the Korean interface face, cut to the Korean catalog (~0.2 MB).
# Both are `glyf` (TrueType) faces from the local craft-fonts checkout.
#
# (family, source face in craft-fonts, locales whose catalogs define the characters,
#  character ranges to keep, the `scripts` tag the app selects the face by)
FACES = (
    ("PdfCraft UI Hans", "fonts/lxgw-wenkai-lite/LXGWWenKaiLite-Regular.ttf",
     ("zh-hans", "zh-hant"), IDEOGRAPH_RANGES, ("Hans", "Hant")),
    ("PdfCraft UI Kore", "fonts/nanum-gothic/NanumGothic-Regular.ttf",
     ("ko",), HANGUL_RANGES, ("Kore",)),
)
APP = "pdfcraft"
# A catalog this small means the file was not parsed, not that the interface has no Chinese:
# shipping such a face would look like success and show tofu for every label.
MIN_CODEPOINTS = 200


def in_ranges(cp: int, ranges) -> bool:
    return any(lo <= cp <= hi for lo, hi in ranges)


def catalog_translations(path) -> str:
    """The rendered text of one catalog: the translation column of every row.

    `craft-i18n` rows are `context <TAB> english <TAB> translation`, with CRLF endings and `#`
    comments. A row with fewer columns is a header or a malformed line, so it is skipped rather
    than guessed at.
    """
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            fields = line.rstrip("\r\n").split("\t")
            if len(fields) < 3 or fields[0].startswith("#"):
                continue
            yield fields[2]


def catalog_codepoints(i18n: pathlib.Path, locales, ranges) -> set:
    """Every codepoint of `ranges` these locales' catalogs render.

    `common.tsv` counts: the app's own rows override the shared ones, and the shared rows are what
    the interface still draws (including the language names the Options picker lists in their own
    script, which is why a Chinese build has to draw Hangul).
    """
    found = set()
    for loc in locales:
        for kind in (APP, "common"):
            path = i18n / "locales" / loc / f"{kind}.tsv"
            if not path.is_file():
                continue
            for text in catalog_translations(path):
                for ch in text:
                    if in_ranges(ord(ch), ranges):
                        found.add(ord(ch))
    return found


def covered_codepoints(path) -> set:
    """Every codepoint a font file maps to a glyph."""
    from fontTools.ttLib import TTFont

    font = TTFont(path)
    cps = set()
    for table in font["cmap"].tables:
        if table.isUnicode():
            cps.update(table.cmap.keys())
    return cps


def build_face(fonts_dir, i18n, out_dir, family, source, locales, ranges, scripts) -> int:
    from fontTools import subset

    src = pathlib.Path(fonts_dir) / source
    if not src.is_file():
        print(f"missing source face: {src} (craft-fonts checkout too old?)", file=sys.stderr)
        return 1
    cps = catalog_codepoints(pathlib.Path(i18n), locales, ranges)
    if len(cps) < MIN_CODEPOINTS:
        print(f"suspiciously few codepoints for {family} from {locales}: {len(cps)}", file=sys.stderr)
        return 1

    opts = subset.Options()
    # No layout features and no hinting: interface labels need plain glyphs, and both are dead
    # weight in a face that will never set body text.
    opts.layout_features = []
    opts.hinting = False
    opts.name_IDs = ["*"]
    opts.notdef_outline = True
    opts.drop_tables += ["GSUB", "GPOS"]

    font = subset.load_font(str(src), opts)
    subsetter = subset.Subsetter(opts)
    subsetter.populate(unicodes=sorted(cps))
    subsetter.subset(font)

    # A derived face carries its own name (OFL), and the name is what the build matches on.
    for record in font["name"].names:
        if record.nameID == 1:
            record.string = family
        elif record.nameID == 2:
            record.string = "Regular"
        elif record.nameID == 4:
            record.string = f"{family} Regular"
        elif record.nameID == 6:
            record.string = family.replace(" ", "") + "-Regular"

    out = pathlib.Path(out_dir) / (family.lower().replace(" ", "-") + ".ttf")
    out.parent.mkdir(parents=True, exist_ok=True)
    subset.save_font(font, str(out), opts)

    # Verify against the file that will ship, not against what the subsetter reported: a codepoint
    # dropped between the two (a cmap format change, an empty glyph) would otherwise surface as
    # tofu in a built package.
    gaps = sorted(cps - covered_codepoints(out))
    if gaps:
        print(
            f"{out}: {len(gaps)} of {len(cps)} interface codepoints have no glyph, e.g. "
            + "".join(chr(c) for c in gaps[:10]),
            file=sys.stderr,
        )
        return 1

    print(
        f"{out}: {len(cps)} codepoints, all covered; {font['maxp'].numGlyphs} glyphs, "
        f"{out.stat().st_size / 1048576:.2f} MB (from {src.stat().st_size / 1048576:.1f} MB), "
        f"scripts {'+'.join(scripts)}"
    )
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--fonts-dir", required=True, help="absolute craft-fonts checkout")
    ap.add_argument("--i18n-dir", default=os.environ.get("CRAFT_I18N_DIR"), help="absolute craft-i18n checkout")
    ap.add_argument("--out-dir", required=True, help="where to write the subset faces")
    args = ap.parse_args()

    if not args.i18n_dir:
        print("no craft-i18n checkout: pass --i18n-dir or set CRAFT_I18N_DIR", file=sys.stderr)
        return 1
    for loc in sorted({loc for _, _, locales, _, _ in FACES for loc in locales}):
        if not (pathlib.Path(args.i18n_dir) / "locales" / loc / f"{APP}.tsv").is_file():
            print(f"missing catalog: {args.i18n_dir}/locales/{loc}/{APP}.tsv", file=sys.stderr)
            return 1

    failed = 0
    for family, source, locales, ranges, scripts in FACES:
        failed |= build_face(args.fonts_dir, args.i18n_dir, args.out_dir, family, source, locales, ranges, scripts)
    return failed


if __name__ == "__main__":
    raise SystemExit(main())
