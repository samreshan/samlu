#!/usr/bin/env bash
# Local macOS (Apple Silicon) build. Produces an unsigned .app/.dmg under
# src-tauri/target/aarch64-apple-darwin/release/bundle/.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if ! command -v cargo >/dev/null; then
  echo "Rust not found. Install it: https://rustup.rs" >&2
  exit 1
fi

if ! rustup target list --installed | grep -q aarch64-apple-darwin; then
  echo "Adding aarch64-apple-darwin target..."
  rustup target add aarch64-apple-darwin
fi

if ! cargo tauri --version >/dev/null 2>&1; then
  echo "Installing tauri-cli..."
  cargo install tauri-cli --version "^2.0.0" --locked
fi

echo "Checking Rust project..."
(cd src-tauri && cargo check)

echo "Building (unsigned)..."
(cd src-tauri && cargo tauri build --target aarch64-apple-darwin)

echo ""
echo "Done. Unsigned build in:"
echo "src-tauri/target/aarch64-apple-darwin/release/bundle/"
echo ""
echo "Gatekeeper will warn on first launch (unsigned build). To run it:"
echo "  right-click the .app -> Open -> Open, or"
echo "  xattr -cr \"/path/to/Samlu.app\""
