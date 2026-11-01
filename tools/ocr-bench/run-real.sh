#!/bin/sh
# Прогон на реальных чеках. Результаты остаются в samples/ и не выходят за пределы Mac.
# Показывать можно только samples/receipts/out/report.json: в нём нет текста чеков.
set -eu
cd "$(dirname "$0")"
R=../../samples/receipts
.cache/venv/bin/python bench.py "$R" --truth "$R/truth" --out "$R/out"
