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
# The runtime reserves 4 GB of shared memory up front, which phones refuse ("RangeError: Out of memory").
# Receipts need under 512 MB (checked with e2e/scan.mjs), so reserve 1 GB. The build stops if the line moved.
ORT_JS="$OUT/ocr/ort-wasm-simd-threaded.mjs"
grep -q 'initial:256,maximum:65536,shared' "$ORT_JS"
sed 's/initial:256,maximum:65536,shared/initial:256,maximum:16384,shared/' "$ORT_JS" > "$ORT_JS.tmp" && mv "$ORT_JS.tmp" "$ORT_JS"

sri() { printf 'sha384-%s' "$(openssl dgst -sha384 -binary "$1" | openssl base64 -A)"; }

# Works with both GNU and BSD sed.
fill() {
  f=$1; shift
  sed "$@" "$f" > "$f.tmp" && mv "$f.tmp" "$f"
}

# A version in every URL: a cache on the way (Cloudflare rewrites Cache-Control for scripts) can then never
# pair a new index.html with an old file, which would fail the integrity check and show a blank page.
ver() { openssl dgst -sha256 -hex "$1" | sed 's/.* //' | cut -c1-12; }

fill "$OUT/boot.js" -e "s|__WASM_SRI__|$(sri "$OUT/web_bg.wasm")|" -e "s|__WASM_V__|$(ver "$OUT/web_bg.wasm")|" -e "s|__JS_V__|$(ver "$OUT/web.js")|"
fill "$OUT/index.html" -e "s|__CSS_SRI__|$(sri "$OUT/app.css")|" -e "s|__JS_SRI__|$(sri "$OUT/web.js")|" -e "s|__BOOT_SRI__|$(sri "$OUT/boot.js")|" \
  -e "s|__CSS_V__|$(ver "$OUT/app.css")|" -e "s|__JS_V__|$(ver "$OUT/web.js")|" -e "s|__BOOT_V__|$(ver "$OUT/boot.js")|"
OCR_VERSION=$(cat "$OUT"/ocr/* | openssl dgst -sha256 -hex | sed 's/.* //' | cut -c1-16)
VERSION=$(cat "$OUT/index.html" "$OUT/boot.js" "$OUT/web.js" "$OUT/web_bg.wasm" "$OUT/ocr-worker.js" | openssl dgst -sha256 -hex | sed 's/.* //' | cut -c1-16)
fill "$OUT/sw.js" -e "s|__VERSION__|$VERSION|" -e "s|__OCR_VERSION__|$OCR_VERSION|"
echo "build $VERSION: app $(du -sh "$OUT/web_bg.wasm" | cut -f1), recognizer $(du -sh "$OUT/ocr" | cut -f1)"
