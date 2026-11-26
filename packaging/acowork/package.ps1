# Build an unsigned .app layout for ACowork (Windows x86_64).
param(
    [string]$OutDir = "",
    [switch]$WithWebUi,
    [string]$WebArchive = "",
    [string]$Platform = "windows-x86_64",
    # Local checkouts the web build reads its interface material from (no network at build time).
    # CRAFT_I18N_DIR supplies the translations; without it the package ships an English-only
    # interface whatever the host asks for. CRAFT_FONTS_DIR supplies the CJK faces.
    [string]$CraftI18nDir = $env:CRAFT_I18N_DIR,
    [string]$CraftFontsDir = $env:CRAFT_FONTS_DIR
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
if (-not $OutDir) {
    $OutDir = Join-Path $Root "dist\acowork\pdfcraft-$Platform"
}

Write-Host "Building pdfcraft-acowork (release)..."
Push-Location $Root
try {
    cargo build -p pdfcraft-acowork --release
    $targetDir = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
} finally {
    Pop-Location
}

$binSrc = Join-Path $targetDir "release\pdfcraft-acowork.exe"
if (-not (Test-Path $binSrc)) {
    throw "missing $binSrc (target_directory=$targetDir)"
}

$stage = $OutDir
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path (Join-Path $stage "bin") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stage "ui") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $stage "assets") | Out-Null

Copy-Item $binSrc (Join-Path $stage "bin\pdfcraft-acowork.exe")

$entry = "bin/pdfcraft-acowork.exe"
$utf8 = New-Object System.Text.UTF8Encoding $false
$tomlPath = Join-Path $PSScriptRoot "app.toml"
$toml = [System.IO.File]::ReadAllText($tomlPath, $utf8)
$toml = $toml -replace 'platforms = \[[^\]]*\]', "platforms = [`"$Platform`"]"
$toml = $toml -replace 'entry = "[^"]*"', "entry = `"$entry`""
[System.IO.File]::WriteAllText((Join-Path $stage "app.toml"), $toml, $utf8)

if ($WebArchive -and $WithWebUi) {
    throw "pass either -WithWebUi or -WebArchive, not both"
}
if ($WebArchive) {
    if (-not (Test-Path $WebArchive)) { throw "missing web archive: $WebArchive" }
    Write-Host "Unpacking prebuilt web UI from $WebArchive"
    $uiDir = Join-Path $stage "ui"
    # tar, not Expand-Archive: pwsh's Expand-Archive resolves System.IO.Compression from the
    # .NET Framework GAC here and dies with "no overload for ExtractToDirectory".
    & (Join-Path $env:SystemRoot "System32/tar.exe") -xf $WebArchive -C $uiDir
    if ($LASTEXITCODE -ne 0) { throw "failed to unpack $WebArchive" }
    $index = Get-ChildItem -Path $uiDir -Filter index.html -Recurse -File | Select-Object -First 1
    if (-not $index) { throw "web archive has no index.html" }
    $indexDir = $index.Directory.FullName
    if ($indexDir -ne (Resolve-Path $uiDir).Path) {
        Get-ChildItem -Force $indexDir | Move-Item -Destination $uiDir
        $nested = $index.Directory
        if ($nested.FullName.StartsWith((Resolve-Path $uiDir).Path) -and $nested.FullName -ne (Resolve-Path $uiDir).Path) {
            Remove-Item -Recurse -Force $nested.FullName
        }
    }
} elseif ($WithWebUi) {
    Write-Host "Building pdfcraft-web via trunk..."
    # The web build embeds its translations and its CJK interface faces from local checkouts. Both
    # are build inputs: without the catalogs the package has no translations at all, and its only
    # CJK face is the Japanese one, which lacks a quarter of the simplified hanzi it renders.
    if (-not $CraftI18nDir) { throw "WithWebUi needs -CraftI18nDir (or CRAFT_I18N_DIR): a package without the catalogs cannot change language" }
    if (-not $CraftFontsDir) { throw "WithWebUi needs -CraftFontsDir (or CRAFT_FONTS_DIR): the web build's CJK faces come from that checkout" }
    $pinned = (Get-Content (Join-Path $Root "craft-i18n.rev") -Raw).Trim()
    $actual = (& git -C $CraftI18nDir rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -eq 0 -and $actual -and $actual.Trim() -ne $pinned) {
        Write-Warning "craft-i18n at $($actual.Trim().Substring(0,8)), this app pins $($pinned.Substring(0,8)): build from a worktree at the pinned rev for a reproducible package"
    }
    $env:CRAFT_I18N_DIR = $CraftI18nDir
    $env:CRAFT_FONTS_DIR = $CraftFontsDir
    # The CJK interface faces, cut to the characters the catalogs above actually render: ~0.8 MB
    # instead of the 13 MB Chinese face the web build drops, and the Korean face it never had.
    # See packaging/acowork/subset-ui-font.py.
    $fontOut = Join-Path (Join-Path $Root "dist") "fonts"
    & python (Join-Path $PSScriptRoot "subset-ui-font.py") --fonts-dir $CraftFontsDir --i18n-dir $CraftI18nDir --out-dir $fontOut
    if ($LASTEXITCODE -ne 0) { throw "the web interface faces failed to build" }
    $env:PDFCRAFT_UI_HANS_SUBSET = (Resolve-Path (Join-Path $fontOut "pdfcraft-ui-hans.ttf")).Path
    $env:PDFCRAFT_UI_KORE_SUBSET = (Resolve-Path (Join-Path $fontOut "pdfcraft-ui-kore.ttf")).Path
    Push-Location (Join-Path $Root "apps\pdfcraft-web")
    try {
        trunk build --release
    } finally {
        Pop-Location
    }
    $webDist = Join-Path $Root "dist\web"
    if (-not (Test-Path $webDist)) { throw "trunk dist missing: $webDist" }
    Copy-Item -Recurse -Force (Join-Path $webDist "*") (Join-Path $stage "ui")
} else {
    Copy-Item -Recurse -Force (Join-Path $PSScriptRoot "ui\*") (Join-Path $stage "ui")
}

$iconCandidates = @(
    (Join-Path $Root "assets\app-icon\hicolor\512x512\apps\ai.storyteller.pdfcraft.png"),
    (Join-Path $Root "assets\icon\icon_512.png"),
    (Join-Path $Root "assets\app-icon\pdfcraft-512.png"),
    (Join-Path $Root "assets\app-icon\pdfcraft-macos-512.png")
)
foreach ($icon in $iconCandidates) {
    if (Test-Path $icon) {
        Copy-Item $icon (Join-Path $stage "assets\icon.png")
        break
    }
}

Copy-Item (Join-Path $Root "LICENSE-MIT") (Join-Path $stage "LICENSE-MIT")
Copy-Item (Join-Path $Root "LICENSE-APACHE") (Join-Path $stage "LICENSE-APACHE")
Copy-Item (Join-Path $PSScriptRoot "NOTICE") (Join-Path $stage "NOTICE")
$iconLicense = Join-Path $Root "assets\app-icon\LICENSE.txt"
if (Test-Path $iconLicense) {
    Copy-Item $iconLicense (Join-Path $stage "assets\ICON-LICENSE.txt")
}

Write-Host "Staged unsigned app tree at $stage"
Write-Host "Next: acowork-pack-app --app-dir `"$stage`" --output `"$stage.app`""
