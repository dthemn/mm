#!/bin/sh
# Builds the client into web/dist without external bundlers, to stay reproducible:
# no inline scripts (strict CSP), SRI on every file.
set -eu
cd "$(dirname "$0")/.."
OUT=web/dist
export RUSTFLAGS="--cfg getrandom_backend=\"wasm_js\" --remap-path-prefix=$PWD=/src --remap-path-prefix=$HOME/.cargo=/cargo"
export SOURCE_DATE_EPOCH=0
cargo build --locked --release --target wasm32-unknown-unknown -p mm-web
rm -rf "$OUT"
mkdir -p "$OUT"
wasm-bindgen --target web --no-typescript --out-dir "$OUT" --out-name web target/wasm32-unknown-unknown/release/web.wasm
cp web/static/* "$OUT/"
rm "$OUT/build.sh" 2>/dev/null || true

sri() { printf 'sha384-%s' "$(openssl dgst -sha384 -binary "$1" | openssl base64 -A)"; }

WASM=$(sri "$OUT/web_bg.wasm")
sed -i '' -e "s|__WASM_SRI__|$WASM|" "$OUT/boot.js" 2>/dev/null || sed -i -e "s|__WASM_SRI__|$WASM|" "$OUT/boot.js"
CSS=$(sri "$OUT/app.css"); JS=$(sri "$OUT/web.js"); BOOT=$(sri "$OUT/boot.js")
sed -i '' -e "s|__CSS_SRI__|$CSS|" -e "s|__JS_SRI__|$JS|" -e "s|__BOOT_SRI__|$BOOT|" "$OUT/index.html" 2>/dev/null \
  || sed -i -e "s|__CSS_SRI__|$CSS|" -e "s|__JS_SRI__|$JS|" -e "s|__BOOT_SRI__|$BOOT|" "$OUT/index.html"
VERSION=$(cat "$OUT/index.html" "$OUT/boot.js" "$OUT/web.js" "$OUT/web_bg.wasm" | openssl dgst -sha256 -hex | sed 's/.* //' | cut -c1-16)
sed -i '' -e "s|__VERSION__|$VERSION|" "$OUT/sw.js" 2>/dev/null || sed -i -e "s|__VERSION__|$VERSION|" "$OUT/sw.js"
echo "build $VERSION: $(du -sh "$OUT" | cut -f1)"
