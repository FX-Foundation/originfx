#!/usr/bin/env bash
# Build the one-file Linux test app: dist/OriginFX-TestApp-linux-x86_64
# Needs the same packages as CI (FFmpeg 6.1 dev libraries, clang, libasound2-dev).
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD

cargo build --release -p originfx-testapp

stage=$(mktemp -d)
cp target/release/originfx-testapp "$stage/"
# Carry every library the app links to, except the C library itself, which must match the system.
ldd target/release/originfx-testapp | awk '/=> \//{print $3}' \
  | grep -vE '/(libc|libm|libdl|libpthread|librt|libutil|libresolv|libnss_[a-z]+)\.so' \
  | while read -r lib; do cp -L "$lib" "$stage/"; done
strip --strip-unneeded "$stage"/*.so* 2>/dev/null || true
echo "carrying $(ls "$stage" | wc -l) files"

python3 packaging/make_zip.py "$stage" "$root/payload.zip"
rm -rf "$stage"

ORIGINFX_PAYLOAD="$root/payload.zip" \
  cargo build --release --manifest-path packaging/launcher/Cargo.toml
mkdir -p dist
cp packaging/launcher/target/release/originfx-launcher dist/OriginFX-TestApp-linux-x86_64
chmod +x dist/OriginFX-TestApp-linux-x86_64
ls -l dist/
