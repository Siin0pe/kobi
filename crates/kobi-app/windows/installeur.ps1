# Construit l'installeur Windows de Kobi (NSIS), avec la CLI `kobi-ctl`
# (KOB-161).
#
#   powershell -ExecutionPolicy Bypass -File crates\kobi-app\windows\installeur.ps1
#
# `kobi-ctl` est un binaire à part (`externalBin`) : il n'est ajouté au paquet
# que par `tauri.installeur.conf.json`, pour que `cargo build`, `cargo test` et
# `clippy` sur kobi-app n'aient pas besoin de lui. L'installeur le copie en
# `bin\kobi.exe` et met `bin` dans le PATH de l'utilisateur
# (`installeur.nsh`).
#
# Respecte CARGO_TARGET_DIR et CARGO_BUILD_JOBS. Installeur produit :
# <target>\release\bundle\nsis\Kobi_<version>_x64-setup.exe

$ErrorActionPreference = 'Stop'
$app = Split-Path $PSScriptRoot -Parent
$racine = (Resolve-Path (Join-Path $app '..\..')).Path

$triple = (rustc -vV | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
$cible = (cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $racine 'Cargo.toml') |
    ConvertFrom-Json).target_directory

Push-Location $racine
try {
    cargo build --release -p kobi-ctl
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p kobi-ctl : échec ($LASTEXITCODE)" }
} finally { Pop-Location }

$binaires = Join-Path $app 'binaries'
New-Item -ItemType Directory -Force $binaires | Out-Null
Copy-Item (Join-Path $cible 'release\kobi-ctl.exe') (Join-Path $binaires "kobi-ctl-$triple.exe") -Force

$tauri = Join-Path $racine 'ui\node_modules\.bin\tauri.cmd'
if (-not (Test-Path $tauri)) { throw "CLI Tauri absente : lancer « npm ci » dans ui\" }
Push-Location $app
try {
    & $tauri build --config tauri.installeur.conf.json --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw "tauri build : échec ($LASTEXITCODE)" }
} finally { Pop-Location }

Get-ChildItem (Join-Path $cible 'release\bundle\nsis') -Filter '*-setup.exe' |
    Select-Object FullName, Length
