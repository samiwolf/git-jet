# Syncs the project version (from version.txt at the repo root) into every
# place it lives: both Cargo.toml manifests, Cargo.lock, tauri.conf.json,
# package.json and package-lock.json.
#
# Usage:  pwsh ./scripts/update-version.ps1   (or run from any directory)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot

if (-not (Test-Path "$root/version.txt")) {
    throw "version.txt not found at $root"
}
$version = (Get-Content "$root/version.txt" -Raw).Trim()
if ($version -notmatch '^\d+\.\d+\.\d+(-[A-Za-z0-9.\-]+)?$') {
    throw "version.txt does not contain a valid version: '$version'"
}

Write-Host "Applying version $version ..." -ForegroundColor Cyan

# Replaces every match of $Pattern with  $Prefix + version + $Suffix  and
# reports how many spots were touched.
function Set-Version {
    param([string]$Path, [string]$Label, [string]$Pattern, [string]$Prefix, [string]$Suffix)

    if (-not (Test-Path $Path)) { throw "$Label not found: $Path" }
    $text = Get-Content $Path -Raw

    $hit = [regex]::Matches($text, $Pattern)
    if ($hit.Count -eq 0) {
        throw "No match for $Label in $Path -- file layout may have changed."
    }

    $replacement = $Prefix + $version + $Suffix
    $updated = [regex]::Replace($text, $Pattern, $replacement)
    [System.IO.File]::WriteAllText($Path, $updated, [System.Text.UTF8Encoding]::new($false))
    Write-Host ("  {0,-28} {1} spot(s) updated" -f $Label, $hit.Count)
}

# 1. Root Cargo.toml  (first standalone version = "..." is the package one)
Set-Version "$root/Cargo.toml" "Cargo.toml" `
    '(?m)^version\s*=\s*"[^"]*"' 'version = "' '"'

# 2. App Cargo.toml
Set-Version "$root/app/src-tauri/Cargo.toml" "app/src-tauri/Cargo.toml" `
    '(?m)^version\s*=\s*"[^"]*"' 'version = "' '"'

# 3. Cargo.lock -- the two workspace package blocks
Set-Version "$root/Cargo.lock" "Cargo.lock (git-jet)" `
    '(?s)(\[\[package\]\]\s*?name = "git-jet"\s*?version = ")[^"]*(")' '${1}' '"'
Set-Version "$root/Cargo.lock" "Cargo.lock (git-jet-app)" `
    '(?s)(\[\[package\]\]\s*?name = "git-jet-app"\s*?version = ")[^"]*(")' '${1}' '"'

# 4. tauri.conf.json (top-level "version")
Set-Version "$root/app/src-tauri/tauri.conf.json" "tauri.conf.json" `
    '(?m)^(\s*)"version"\s*:\s*"[^"]*"' '${1}"version": "' '"'

# 5. package.json (top-level "version" -- the only place with that key)
Set-Version "$root/app/package.json" "package.json" `
    '(?m)^(\s*)"version"\s*:\s*"[^"]*"' '${1}"version": "' '"'

# 6. package-lock.json -- both entries of the root package
Set-Version "$root/app/package-lock.json" "package-lock.json" `
    '(?s)("name": "git-jet",\s*?"version": ")[^"]*(")' '${1}' '"'

Write-Host ""
Write-Host "All version references set to $version" -ForegroundColor Green
