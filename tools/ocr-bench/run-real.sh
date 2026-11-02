#!/bin/sh
# Runs on real receipts. Results stay in samples/ and never leave this Mac.
# Only samples/receipts/out/report.json may be shared: it has no receipt text.
set -eu
cd "$(dirname "$0")"
R=../../samples/receipts
.cache/venv/bin/python bench.py "$R" --truth "$R/truth" --out "$R/out"
