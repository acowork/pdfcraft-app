# ACowork hosted adaptation

This repository is the **ACowork fork** of [storytold/pdfcraft](https://github.com/storytold/pdfcraft).

Goal: ship a supervised hosted app (ADR-116) that Desktop can install via the
[ACoworkAppGallery](https://github.com/acowork/ACoworkAppGallery) entry
`apps/acowork/pdfcraft` and open inside an iframe.

## Name and marks

The hosted app **id** stays `pdfcraft`. The public name is **PdfCraft (ACowork)**
so it is not presented as an ArtCraft product.

- You may say, in plain text, that this work is based on PdfCraft by the ArtCraft team.
- Do **not** ship or display the ArtCraft name, wordmark or logos (`docs/brand/` is not open source; see upstream `docs/brand/LICENSE-brand.txt`).
- The PdfCraft **app icon** (`assets/app-icon/`) is original PdfCraft artwork under MIT OR Apache-2.0. That is what the package and gallery use — not the ArtCraft mark.
- Homepage for this adaptation is this fork. Upstream stays `https://github.com/storytold/pdfcraft`.
- Every `.app` tree copies `LICENSE-MIT`, `LICENSE-APACHE`, `packaging/acowork/NOTICE`, and `assets/ICON-LICENSE.txt`.

## Layout

| Path | Role |
|---|---|
| [`apps/pdfcraft-acowork`](apps/pdfcraft-acowork) | Loopback host binary: `/health`, `/ui/*`, `POST /mcp` |
| [`packaging/acowork`](packaging/acowork) | `app.toml` + stage scripts for an unsigned `.app` tree |
| [`apps/pdfcraft-web`](apps/pdfcraft-web) | Upstream Trunk/WASM UI (copied into package `ui/` when ready) |

- Visual canvas: workspace `*.pdf` (and related extensions) + `doc_open` / `doc_save`.

## In-iframe bridge (ADR-118 / design doc 35)

ACowork opens this app in a sandboxed iframe (no `allow-same-origin`) and injects a small
script into `/ui/` ([`apps/pdfcraft-acowork/src/ui.rs`](apps/pdfcraft-acowork/src/ui.rs)). It:

- sets `data-host="page"` on the canvas before the WASM app starts, so the app's own
  `craft-web-host` bridge listens on the canvas — the `?host=parent` transport can never
  connect from the sandboxed (opaque-origin) iframe;
- maps ACowork's theme message to `view.theme.light` / `view.theme.dark`;
- maps ACowork's locale message to `ui.language` `{ value: "zh-hans" }`, and reads `?lang=` at
  startup: the host's interface language is not the browser's, so an embedded PdfCraft opens —
  and switches — in the language the user set in ACowork. `ui.language` is not an engine command;
  `crates/ui-egui/src/control/requests.rs` routes it to the same preference the Appearance dialog
  writes, and lists it in `ui.commands` so a host can discover it;
- delivers the workspace file as the app's `pdfcraft:open` event (`{name, bytes}`);
- forwards `pdfcraft:save` to the Desktop, which writes the file back through
  `PUT /api/agents/{id}/workspaces/file` (`contentBase64`) and answers `saved`.

Workspace extensions this build claims (`packaging/acowork/app.toml`
`[ui.canvas] extensions`): `pdf`. Installing the app takes those formats over from the
built-in viewers; uninstalling gives them back.

## Build a package tree (Windows)

```powershell
# Placeholder UI (fast) — enough to exercise install + iframe shell
.\packaging\acowork\package.ps1

# Real web UI built from this tree (needs trunk + wasm32 target)
.\packaging\acowork\package.ps1 -WithWebUi
```

`-WithWebUi` needs the two local checkouts the web build reads its interface material from; a
package built without them is a different program - no translations, and a Chinese interface with
boxes where the Japanese face has no simplified hanzi:

```powershell
.\packaging\acowork\package.ps1 -WithWebUi `
    -CraftI18nDir E:\projects\tranxon\craft-i18n -CraftFontsDir E:\projects\tranxon\craft-fonts
```

`craft-i18n` should be at the rev in `craft-i18n.rev` (`git worktree add` a second checkout keeps
several apps' pins side by side). The script cuts the web build's Chinese and Korean interface
faces down to the characters the catalogs actually render (`packaging\acowork\subset-ui-font.py`,
~0.8 MB instead of the 13 MB face native builds carry).

Then, from the ACowork monorepo (or a release of `acowork-pack-app`):

```text
acowork-pack-app --app-dir <staged-dir> --output pdfcraft-<ver>-windows-x86_64.app
# optional: sign with acowork-sign, publish GitHub Release, set gallery package.url + sha256
```

Until a signed Release exists, Gallery `package` stays `null`. Unsigned installs need
`apps.allow_unsigned = true` or `dev_mode`.
