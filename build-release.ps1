<#
.SYNOPSIS
    Build, sign and (optionally) deploy a release build with no build-machine
    paths baked into the binary.

.DESCRIPTION
    Two things leak local paths into a plain `npm run tauri build`:

      1. The linker writes a CodeView debug directory entry naming
         sticky_notes_lite.pdb. Handled by `strip = true` in
         src-tauri/Cargo.toml, plus /DEBUG:NONE here as a belt-and-braces
         measure since Cargo's `strip` is a no-op on some MSVC setups.

      2. rustc bakes the absolute source path of every crate into its
         panic-location strings -- 370 C:\Users\<you>\.cargo\... and 165
         C:\Users\<you>\.rustup\... occurrences were measured in .rdata, which
         is ordinary read-only data, NOT debug info. `strip` cannot remove
         these; only --remap-path-prefix can. Cargo's `trim-paths` profile key
         would be the tidier fix but is still unstable as of Cargo 1.98.

    The prefixes are read from the environment rather than hardcoded, so no
    username ends up committed to the repo.

    Note this also means no .pdb is produced, so a future crash dump resolves
    only to module+offset. That is what the deadlock investigation in README.md
    worked from anyway, so it is an accepted trade-off rather than a new
    limitation.

.PARAMETER Deploy
    After signing, stop the running app, back up dist\sticky-notes-lite.exe and
    replace it with the new build, then relaunch.

.PARAMETER SkipSign
    Build only. Useful for a quick check without touching the certificate.
#>
[CmdletBinding()]
param(
    [switch]$Deploy,
    [switch]$SkipSign
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$exe = Join-Path $root "src-tauri\target\release\sticky-notes-lite.exe"
$dist = Join-Path $root "dist\sticky-notes-lite.exe"
$thumbprint = "2FD74642CE56E472D2CB807D396B77DA651FEF41"

# --- path remapping ---------------------------------------------------------
$cargoHome  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { Join-Path $env:USERPROFILE ".cargo" }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE ".rustup" }
$cargoHome  = $cargoHome.TrimEnd('\')
$rustupHome = $rustupHome.TrimEnd('\')

$flags = @(
    "--remap-path-prefix=$cargoHome=[cargo]"
    "--remap-path-prefix=$rustupHome=[rustup]"
    "--remap-path-prefix=$root=[src]"
    "-Clink-arg=/DEBUG:NONE"
)
$env:RUSTFLAGS = $flags -join " "
Write-Host "RUSTFLAGS = $env:RUSTFLAGS" -ForegroundColor DarkGray

# Changing the icon requires clearing this, and RUSTFLAGS changes invalidate
# the cache anyway -- see "Changing the app icon" in README.md.
Get-ChildItem (Join-Path $root "src-tauri\target\release\build") -Filter "sticky-notes-lite-*" -Directory -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force

Push-Location $root
try {
    # npm writes notices to stderr, which PowerShell turns into a terminating
    # NativeCommandError under ErrorActionPreference=Stop even on success --
    # so judge the build by its exit code, not by whether it wrote to stderr.
    $ErrorActionPreference = "Continue"
    & npm run tauri build -- --no-bundle
    $code = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    if ($code -ne 0) { throw "build failed with exit code $code" }
} finally {
    Pop-Location
}
if (-not (Test-Path $exe)) { throw "expected binary not found: $exe" }

# --- sign -------------------------------------------------------------------
if (-not $SkipSign) {
    $signtool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" |
        Sort-Object FullName | Select-Object -Last 1
    if (-not $signtool) { throw "signtool.exe not found" }
    & $signtool.FullName sign /fd SHA256 /sha1 $thumbprint `
        /tr http://timestamp.digicert.com /td SHA256 $exe
    if ($LASTEXITCODE -ne 0) { throw "signing failed" }
}

# --- deploy -----------------------------------------------------------------
if ($Deploy) {
    Get-Process -Name "sticky-notes-lite" -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 3
    if (Test-Path $dist) {
        Copy-Item $dist "$dist.bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')" -Force
    }
    Copy-Item $exe $dist -Force
    Start-Process -FilePath $dist | Out-Null
    Write-Host "deployed and relaunched" -ForegroundColor Green
}

Write-Host "done: $exe" -ForegroundColor Green
