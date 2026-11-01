#!/bin/sh
# Ставит движки и модели в .cache (папка в .gitignore). Сеть нужна только здесь.
set -eu
cd "$(dirname "$0")"
mkdir -p .cache/models
python3.14 -m venv .cache/venv
.cache/venv/bin/pip install -q pillow pillow-heif numpy onnxruntime rapidocr
for k in fast best; do
  curl -fsSL -o ".cache/models/pol_$k.traineddata" \
    "https://github.com/tesseract-ocr/tessdata_$k/raw/main/pol.traineddata"
done
[ -f .cache/package.json ] || echo '{"name":"ocr-bench-deps","private":true}' > .cache/package.json
(cd .cache && npm i --silent tesseract-wasm onnxruntime-web)
cargo install ocrs-cli --version 0.13.1 --root .cache/ocrs --locked
