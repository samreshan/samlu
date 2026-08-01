#!/usr/bin/env bash
# Local macOS (Apple Silicon) build. Uses APPLE_SIGNING_IDENTITY when supplied
# and falls back to an ad-hoc signature for local testing.
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

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  signing_label="identity: $APPLE_SIGNING_IDENTITY"
else
  export APPLE_SIGNING_IDENTITY="-"
  signing_label="ad-hoc local signature"
  echo "Warning: no Apple signing identity supplied." >&2
  echo "Accessibility and microphone grants may need to be re-added after this build changes." >&2
fi

echo "Building ($signing_label)..."
(cd src-tauri && cargo tauri build --target aarch64-apple-darwin --bundles app)

app_path="$root/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Samlu.app"
app_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app_path/Contents/Info.plist")"
dmg_path="$root/src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/Samlu_${app_version}_aarch64.dmg"
staging_dir="$(mktemp -d "${TMPDIR:-/tmp}/samlu-dmg.XXXXXX")"

cleanup() {
  if [[ "$staging_dir" == */samlu-dmg.* && -d "$staging_dir" ]]; then
    rm -rf "$staging_dir"
  fi
}
trap cleanup EXIT

mkdir -p "$(dirname "$dmg_path")"
cp -R "$app_path" "$staging_dir/Samlu.app"
ln -s /Applications "$staging_dir/Applications"
hdiutil create -volname Samlu -srcfolder "$staging_dir" -ov -format UDZO "$dmg_path"
hdiutil verify "$dmg_path"

echo ""
echo "Done. Build in:"
echo "src-tauri/target/aarch64-apple-darwin/release/bundle/"
echo ""
if [[ "$APPLE_SIGNING_IDENTITY" == "-" ]]; then
  echo "This local build is ad-hoc signed and not notarized."
  echo "Gatekeeper may warn on first launch; right-click the app and choose Open."
else
  echo "Signed with: $APPLE_SIGNING_IDENTITY"
fi
