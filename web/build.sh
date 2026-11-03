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
web/ocr-assets.sh
mkdir -p "$OUT/ocr"
cp web/ocr/cache/det.onnx web/ocr/cache/rec.onnx web/ocr/cache/ort.wasm.min.mjs \
  web/ocr/cache/ort-wasm-simd-threaded.mjs web/ocr/cache/ort-wasm-simd-threaded.wasm web/ocr/latin_dict.txt "$OUT/ocr/"

sri() { printf 'sha384-%s' "$(openssl dgst -sha384 -binary "$1" | openssl base64 -A)"; }

# Works with both GNU and BSD sed.
fill() {
  f=$1; shift
  sed "$@" "$f" > "$f.tmp" && mv "$f.tmp" "$f"
}

fill "$OUT/boot.js" -e "s|__WASM_SRI__|$(sri "$OUT/web_bg.wasm")|"
fill "$OUT/index.html" -e "s|__CSS_SRI__|$(sri "$OUT/app.css")|" -e "s|__JS_SRI__|$(sri "$OUT/web.js")|" -e "s|__BOOT_SRI__|$(sri "$OUT/boot.js")|"
VERSION=$(cat "$OUT/index.html" "$OUT/boot.js" "$OUT/web.js" "$OUT/web_bg.wasm" | openssl dgst -sha256 -hex | sed 's/.* //' | cut -c1-16)
fill "$OUT/sw.js" -e "s|__VERSION__|$VERSION|"
echo "build $VERSION: app $(du -sh "$OUT/web_bg.wasm" | cut -f1), recognizer $(du -sh "$OUT/ocr" | cut -f1)"
