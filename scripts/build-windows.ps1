# Local Windows build. Produces an unsigned .msi/.nsis installer under
# src-tauri\target\release\bundle\.
$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  Write-Error "Rust not found. Install it: https://rustup.rs"
  exit 1
}

if (-not (Get-Command "cargo-tauri" -ErrorAction SilentlyContinue)) {
  Write-Host "Installing tauri-cli..."
  cargo install tauri-cli --version "^2.0.0" --locked
}

Write-Host "Checking Rust project..."
Push-Location src-tauri
try {
  cargo check
} finally {
  Pop-Location
}

Write-Host "Building (unsigned)..."
Push-Location src-tauri
try {
  cargo tauri build
} finally {
  Pop-Location
}

Write-Host ""
Write-Host "Done. Unsigned build in:"
Write-Host "src-tauri\target\release\bundle\"
Write-Host ""
Write-Host "Windows SmartScreen will warn on first launch (unsigned build)."
