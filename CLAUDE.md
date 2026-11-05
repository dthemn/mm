# mm: project notes for Claude

Read this first in every session. It holds what the code and git history do not.

## The owner and how to work with them

- The owner writes in Russian. Answer in Russian. Everything in the repository is in English:
  code, comments, docs, tests, commits, and the app's interface. This replaced the Russian UI of the original spec.
- Work in stages. After each stage, show the result and stop for review. Do not start the next stage unprompted.
- Never call something done without running the tests, and say plainly what was not verified,
  especially anything that needs a real phone.
- The owner usually wants finished work pushed to `main` on https://github.com/dthemn/mm (public).
- Ask instead of guessing when the spec contradicts itself or a check needs the owner's samples.

## What the app is

A family income and expense tracker for Poland (PLN), as a mobile PWA in Rust with end-to-end encryption.
Each family member has a separate account and key. Main flow: photograph a receipt, the app does the rest.

Principles, highest priority first:

1. The server never sees plaintext. The database holds only ciphertext.
2. Recognition, parsing and analytics run only on the device. Photos and PDFs never reach the server.
3. At most 3 taps from opening the app to a saved receipt.
4. Reliability over features: exact money in integer grosze, and sync must never lose data.

## Decisions already made

- Sign-in is a passkey. The 12 BIP39 words are only a backup for a lost passkey, and are needed to add a passkey.
- A passkey without PRF: the 12 words are entered once on that device, then the DEK is kept under a
  non-extractable WebCrypto key, and later sign-ins use the passkey.
- P-256 from RustCrypto signs phrase logins, instead of Ed25519, to stay within vetted crates.
- `revision` is in the AAD and an encrypted manifest detects server rollbacks. Both go beyond the original spec.
- Record ids derived from names (merchants, category rules) are HKDF-keyed with the DEK, never plain hashes:
  the server sees record ids and could otherwise test names from a dictionary.
- NIP is always optional. A wrong check digit warns but never blocks saving. Dashes, spaces and `PL` are accepted.
- OCR is PP-OCR v5 mobile (Latin) in ONNX Runtime Web, inside a Web Worker. `ocrs` has no Polish letters,
  and Tesseract misread totals. ORT is an accepted exception to "Rust everywhere".
- Look: monospace type, dashed rules, receipt-slip cards. Dark theme by default; the user's theme is an encrypted
  `prefs` record (id from `named_id`), cached in `localStorage.mmTheme` only so the sign-in screen opens in it.
- Home has Daily, Monthly and Summary tabs (`FilterState.tab`); tapping the month opens a calendar sheet that can jump
  to any year, month or day. Stats keeps the trend, top merchants and subscriptions.
- The camera is a full-screen overlay. The light button shows only when `getCapabilities().torch` is true; whether
  iPhone Safari offers it is unverified.
- The operation form is a receipt: label left, value right, dashed rules. On a new operation an upload button takes
  a receipt photo (recognized), a CSV statement (opens the import preview via `Ctx.upload`) or a PDF (not read yet, says so).
- "Stay signed in" (Settings, per device in `localStorage.mmKeep`: 1 min, 1 hour, 12 hours; default 12 hours, the owner's
  choice): the DEK is kept under a separate non-extractable WebCrypto key (`keep:` in `mm-device`), valid only while
  `localStorage.mmKeepUntil` matches, so Lock revokes it at once. iOS drops background pages, so memory alone is not enough.
  Test devices start with 1 min.
- Screenshots of e-receipts in a dark app (light text on dark) are inverted before recognition, not cropped to
  their white barcode panel (`vision::light_on_dark`). Private sample in `samples/screens/` (git-ignored).
- Stats search (`analytics::search`): every word must appear in an item name, category, merchant or note; it adds up
  purchases, quantity per unit and money, for the period or all time. Suggestions come from the user's own data.
- Only `main` scrolls; the page itself does not, so the tab bar (icons with small captions) stays at the bottom on iOS.
- Income has its own built-in categories (`Category.income`): Salary, Bonus, Cashback, Gift, Refund, Other income
  (ids `builtin(20..25)`). The form lists the ones matching the kind; statement rows get one from `income_category`.
- A receipt keeps one operation; `analytics::groups` sorts its lines by category with exact sums. Home shows the
  groups under a mixed receipt, the detail lists lines under their category.
- Stats: Income/Expenses tabs, a ring by category (8 validated hues per theme, gray for Other, color fixed per
  category), and a tapped category leaves every number on that screen (`Filter.exclude`).
- Home Summary shows rings by category for expenses and income; a tap opens the category. Categories, merchants and
  accounts are receipt cards (`.slipcard`), one row per entry with a badge and its use, opening in place to edit.
  (`.sheet` is the calendar bottom sheet, do not reuse it.)
- Categories are English: Food, Household, Subscriptions, Transport, Health, Clothing, Entertainment,
  Cafes & restaurants, Other. Built-ins have fixed ids. Merging sets `merged_into` instead of rewriting records.

## Status (2026-11-03)

Stage 5 so far: `core::import` (CSV, Windows-1250, column guessing, stable keyed record ids so a re-import adds
nothing), `core::dedup` (receipt and bank operation, own-account transfers, recurring payments), and a server proxy
for NBP rates at `/api/rates/{code}/{year}/{quarter}` with a cache. Bank profiles wait for real statements, which the
owner has not added yet. Next: PDF text import, password-protected PDFs, invoices, then bank profiles.

| Stage | State |
|---|---|
| 1. OCR comparison, architecture, threat model | done |
| 2. Workspace, passkey login, encryption, sync | done |
| 3. Manual operations, dashboard, export | done |
| 4. Receipt from photo to saved operation | works in desktop Chrome; not tried on a phone |
| 5. Bank statement and invoice import, deduplication | CSV import, dedup, transfers, NBP rates, subscriptions done; bank profiles, PDF and invoices not started |
| 6. Subscriptions, PWA polish, backups, deployment | deploy files written, not run on a VPS |

On 2026-11-04 the owner registered with a passkey and synced on an iPhone 13 at https://mm.dthem.com: the app runs
in Docker on the owner's Mac (compose.yml, tunnel profile, `.env` holds the tunnel token). The camera, PRF, install
to home screen and OCR speed on a mid-range Android are still unverified. No VPS yet.

## Layout

- `core/` money, dates, models, analytics, export, crypto, sync, `vision` (photo prep), `ocr` (model I/O),
  `receipt` (Polish fiscal receipt parser), `categorize`. Tested natively, no browser.
- `server/` Axum, sqlx, PostgreSQL. Stores ciphertext only. `mm-server invite` creates an invite.
- `web/` Leptos CSR. `app.rs` auth shell, `finance.rs` screens (Home lists operations by day, Stats holds the charts,
  `Screen::Tx` shows one operation with its receipt lines), `scan.rs` camera and OCR, `data.rs` decrypted cache.
  `web/build.sh` builds `web/dist` with SRI; `web/ocr-assets.sh` downloads the recognizer and checks pinned hashes.
- `e2e/` Chrome tests with a virtual passkey: `run.mjs` (main suite), `scan.mjs` (scores real receipts),
  `smoke.mjs` (checks a running deployment), `fixtures/receipt.jpg` (synthetic, made up).
- `deploy/` production compose with Caddy, `docs/` architecture, threat model, deploy guide.
- `tools/ocr-bench/` stage 1 OCR comparison.

## Running and testing

```sh
docker compose -f compose.dev.yml up -d        # throwaway test DB on 127.0.0.1:55432
cargo test -p mm-core                          # 158 tests
cargo test -p mm-server                        # 21 tests, needs the test DB
web/build.sh && cargo build -p mm-server
node e2e/run.mjs > /tmp/e2e.log 2>&1           # 56 tests; write to a file, never pipe into head or tail
LIMIT=90000 node e2e/scan.mjs                  # real receipts in samples/receipts, prints scores only
SHOW=1 ...                                     # also prints recognized lines (sets localStorage.mmDebug)
docker compose up --build -d                   # whole app on http://localhost:8080 (use localhost, not 127.0.0.1)
```

Gotchas learned the hard way:

- Long test commands must write to a file. A spawned test server once kept a pipe open and looked like an app hang.
- `cargo clippy --workspace --all-targets` must stay at zero warnings.
- A `move ||` view closure in Leptos that reads a signal rebuilds its children: never put a button whose handler
  sets that signal inside it.
- IndexedDB `deleteDatabase` never finishes while a connection is open, so storage is closed on every lock.
- Docker Desktop on this Mac can stall when binding ports 80 and 443. Test the prod stack with
  `HTTP_PORT=8088 HTTPS_PORT=8443 PUBLIC_ORIGIN=https://localhost:8443` in `deploy/.env`.
- The production compose project is `mm-prod`; the dev one is `mm`. They used to collide.
- The auto-mode safety check blocks `git reset --hard`. Resolve merges by hand instead.
- iPhone needs a real https address with a domain: plain http on a LAN IP disables passkeys, the camera and Web Locks,
  and Safari can wait forever without a passkey sheet on a certificate it does not trust (hence the 90 s limit).
- Static files are served with `Cache-Control: no-cache`, and `web/build.sh` adds `?v=<hash>` to every app URL:
  Cloudflare rewrites Cache-Control for scripts to 4 hours, and a new index.html meeting an old script fails SRI.
- ONNX Runtime Web reserves 4 GB of shared wasm memory, which phones refuse ("RangeError: Out of memory").
  `web/build.sh` lowers the maximum to 1 GB; receipts need between 256 and 512 MB. The OCR cache in `sw.js` is named
  after the recognizer files' hash, so a changed file reaches phones that cached the old one.
- An invite is deleted when registration finishes, and checked on the invite step via `/api/auth/invite/check`.

## Data and privacy rules

- Real receipts are in `samples/receipts/` with ground truth in `samples/receipts/truth/`. The owner allowed
  reading them. Bank statements in `samples/statements/` must not be read without asking first.
- `samples/` is git-ignored. Only anonymized or synthetic data goes into the repository and tests.
- Before every commit, check the staged list for `samples/`, `*.jpg` other than the fixture, `*.onnx`, `*.wasm`,
  `web/ocr/cache/`, `dist/`, `deploy/.env`. On 2026-11-03 the OCR models slipped into `main` once
  (removed in `a51cb8c`, still in history; rewriting history needs the owner's consent).

## Known gaps

- Accuracy targets (total 95%, merchant and date 90%, items 85%) are unproven: only 3 real receipts exist,
  and they were used for tuning. Needs 30 to 50 receipts with a held-out third.
- No CI yet with `cargo audit` and `cargo deny`.
- No backups. Restoring an old database lowers `seq` below device cursors, so a restore epoch is needed first.
- No bank-specific statement profiles, no PDF import, no invoices yet.
- A second open tab blocks erasing local data until it closes.
- Lines of the original spec were cut off when pasted: the end of the category list, the `Transaction` fields
  between payment method and source, and minimum iOS and Android versions. The current choices are guesses.

## Stage 5 requirements (from the original spec)

- On-device import, with a preview before saving. One format profile per bank, plus a generic column mapping
  wizard and CSV. Banks: PKO BP, Pekao, mBank, ING, Santander, Millennium, Alior, BNP Paribas, Credit Agricole,
  VeloBank, Nest Bank, Revolut. A bank counts as supported only with a test on a real statement from `samples/statements/`.
- PDFs with a text layer, scans through the same OCR, and password-protected PDFs.
- Revolut is multi-currency: keep the original amount, currency and PLN equivalent. Rates from the public NBP API,
  requested by quarter, not per transaction date.
- Invoices (PDF or photo): seller, NIP, number, dates, items, net, VAT, gross. KSeF is out of scope.
- Deduplication is mandatory: re-importing a statement creates no duplicates; a receipt and a bank operation with the
  same amount on a close date count once, with the receipt items as the operation's detail; doubtful matches go to the user.
- Transfers between one's own accounts are neither income nor expense. Recurring payments are detected and offered as subscriptions.

## Stage 6 requirements (from the original spec)

- Subscriptions view, interface polish, full PWA, deployment.
- HTTPS, HSTS, strict CSP and login rate limiting are in place. Still needed: encrypted database backups with a
  tested restore, `cargo audit` and `cargo deny` in CI, dependency updates.
- README must keep the honest note on web E2EE: whoever controls the served frontend could replace the code.
