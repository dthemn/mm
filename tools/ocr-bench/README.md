# OCR engine comparison

The stage 1 bench. It runs locally and photos go nowhere. Only `setup.sh` uses the network.

```sh
./setup.sh                  # once
./run-real.sh               # receipts from samples/receipts/, ground truth from samples/receipts/truth/
node phone/serve.mjs 8099   # speed test for a phone (see below)
```

Only `samples/receipts/out/report.json` and `per_receipt.json` are safe to share: they hold flags and percentages,
no receipt text. Recognized text stays in `samples/receipts/out/<engine>/`.

## Ground truth

Each photo `samples/receipts/<name>.jpg` needs `samples/receipts/truth/<name>.json`. Amounts are in grosze.

```json
{
  "merchant": "Name as printed",
  "nip": "1234567890",
  "date": "2026-10-31",
  "total": 19769,
  "items": [
    {"name": "Name as printed", "qty": "1,614", "price": 2183, "value": 3523, "discount": 0}
  ]
}
```

## Metrics

- `total`, `date`, `nip`: exact match.
- `merchant`: similarity of at least 0.85 with one of the first ten lines.
- `items`: share of receipt items where name (similarity of at least 0.8, ignoring diacritics), quantity, price and value are all right.
- `polish`: share of the letters ą ć ę ł ń ó ś ź ż (and capitals) from the ground truth names that were read exactly.

The parser in `bench.py` is a prototype used only for scoring. The real one arrives in `core` at stage 4.
`raw` means deskewing only, and `prep` adds light flattening and contrast stretching.

## Speed on a phone

```sh
BENCH_IMG=<path to a synthetic receipt> node phone/serve.mjs 8099
```

On a phone on the same Wi-Fi network, open `http://<Mac IP>:8099/?th=1`. Results land in `phone/results.jsonl`.
Over plain HTTP the page is not a secure context, so threads are unavailable and `isolated` is `false`.
This gives a worst-case single-thread number. A multi-thread number needs HTTPS.

`synth.py` output only checks the bench itself. It never counts toward the accuracy targets.
