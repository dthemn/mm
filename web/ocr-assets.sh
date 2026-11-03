#!/bin/sh
# Downloads the recognizer once and checks every file against a pinned hash.
# A file that does not match is deleted, and the build stops.
set -eu
cd "$(dirname "$0")"
DIR=ocr/cache
mkdir -p "$DIR"

ORT_VERSION=1.31.1
ORT_SHA512=5GeH+bWQ+HxM10z7erGlsWqryrwMbSvm/1tkIsKTT48g0fI19VXVVC435t4t7VsPdSQeS7NmuZPXrQ73k82URw==
DET_SHA256=4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae
REC_SHA256=b20bd37c168a570f583afbc8cd7925603890efbcdc000a59e22c269d160b5f5a
MODELS=https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.10.0/onnx/PP-OCRv5

sha256() { openssl dgst -sha256 -hex "$1" | sed 's/.* //'; }

fetch() { # url file sha256
  if [ ! -f "$DIR/$2" ] || [ "$(sha256 "$DIR/$2")" != "$3" ]; then
    curl -fsSL --retry 3 -o "$DIR/$2.part" "$1"
    if [ "$(sha256 "$DIR/$2.part")" != "$3" ]; then
      rm -f "$DIR/$2.part"
      echo "hash mismatch for $2" >&2
      exit 1
    fi
    mv "$DIR/$2.part" "$DIR/$2"
  fi
}

fetch "$MODELS/det/ch_PP-OCRv5_det_mobile.onnx" det.onnx "$DET_SHA256"
fetch "$MODELS/rec/latin_PP-OCRv5_rec_mobile.onnx" rec.onnx "$REC_SHA256"

if [ ! -f "$DIR/ort.wasm.min.mjs" ]; then
  TGZ="$DIR/ort.tgz"
  curl -fsSL --retry 3 -o "$TGZ" "https://registry.npmjs.org/onnxruntime-web/-/onnxruntime-web-$ORT_VERSION.tgz"
  if [ "$(openssl dgst -sha512 -binary "$TGZ" | openssl base64 -A)" != "$ORT_SHA512" ]; then
    rm -f "$TGZ"
    echo "hash mismatch for onnxruntime-web" >&2
    exit 1
  fi
  tar -xzf "$TGZ" -C "$DIR" --strip-components=2 \
    package/dist/ort.wasm.min.mjs package/dist/ort-wasm-simd-threaded.mjs package/dist/ort-wasm-simd-threaded.wasm
  rm "$TGZ"
fi
