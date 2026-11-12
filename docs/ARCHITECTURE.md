# Architecture and threat model

Status: stages 1 to 4. Receipt recognition uses PP-OCR v5 mobile and was tuned on only 3 real receipts.

Owner decisions:

- Daily sign-in uses only the passkey. The 12 words are a backup for a lost passkey.
- Each family member has a separate account and DEK. There is no shared data or shared key.
- The whole app, including its interface, is in English. This replaces the Russian interface of the original requirements.

## 1. Components

```
Phone (PWA)                                     VPS in the EU
┌────────────────────────────────────┐         ┌───────────────────────┐
│ web: Leptos CSR, service worker    │         │ Caddy: TLS, HSTS      │
│ core (WASM): crypto, merge, sync   │  HTTPS  │ server: Axum, sqlx    │
│ OCR worker (stage 4)               │ ──────► │ PostgreSQL: ciphertext│
│ IndexedDB: ciphertext and queue    │         │ and minimal metadata  │
└────────────────────────────────────┘         └───────────────────────┘
```

All parsing, analytics and deduplication run on the client. The server has no code able to read records.

## 2. Keys

| Key | Source | Where it lives |
|---|---|---|
| DEK, 256 bits | random, at registration | client memory only; the server holds wrapped copies |
| KEK_prf | HKDF(WebAuthn PRF output, info `mm/v1/kek/prf`) | derived at sign-in |
| KEK_phrase | HKDF(BIP39 entropy, info `mm/v1/kek/phrase`) | derived during recovery |
| Phrase login key | HKDF(BIP39 entropy, info `mm/v1/auth/phrase`), P-256 ECDSA | the server has only the public part |

- Different `info` values make the keys independent: the public key and signatures reveal nothing about KEK_phrase.
- 12 words carry 128 bits of entropy, so the DEK copy under the phrase has 128-bit strength. This is enough.
  HKDF without stretching is acceptable only because the phrase is random, so user-chosen phrases are never accepted.
- P-256 from RustCrypto is used instead of Ed25519 to stay within the vetted crate list.
- The PRF salt is the same for the whole app, because the user is unknown during username-less login.
  This is safe: each passkey produces its own PRF output.
- Each passkey has its own wrapped DEK copy. The server returns a copy only after authentication.
- The PRF output never leaves the device. An e2e test checks this on the actual requests.
- `webauthn-rs` has no PRF support, so the browser code adds the extension. It also requests a discoverable
  passkey, which allows login on a new device without a username.
- Limitation: `zeroize` clears Rust and WASM memory, but the PRF output passes through JavaScript and the browser,
  where clearing is not guaranteed. Plaintext shown on screen is in the same position. Clearing shortens the
  exposure window but does not remove it.

Sign-in modes:

- Normal: a passkey with PRF unwraps the DEK. No phrase is needed.
- Offline: a local WebAuthn request produces the PRF output, which unwraps a locally cached copy of the wrapped DEK.
  No signature is verified, and the proof is that decryption succeeds.
- Lost passkey: the phrase signs in and unwraps the DEK, then the user registers a new passkey.
- Passkey without PRF: the phrase is entered once when linking the device. The DEK is then stored locally under a
  non-extractable WebCrypto key, and later sign-ins use the passkey. This is weaker: the browser protects that copy,
  not the biometric check, so code with access to the browser profile could unwrap it.
- Passkey saved without its PRF copy: some authenticators (Bitwarden on an iPhone did) return no PRF when the passkey
  is created, but do return it at sign-in, on that device or another one the password manager syncs to. When a sign-in
  gives PRF and the server holds no copy for that passkey, the DEK is unlocked once with the phrase (or the device's
  stored copy), then wrapped under the PRF and saved with `POST /api/passkeys/prf`. The server accepts it only for the
  session user's own passkey and only while it has none, so an existing copy is never replaced. Later sign-ins on any
  device where the passkey gives PRF need no phrase.

Adding a passkey requires a phrase signature in addition to a session, so a stolen cookie is not enough.

## 3. Records and sync

```
server:     user_id, record_id (random), revision, seq, size, updated_at, blob
blob:       format_version | nonce (24 bytes) | ciphertext
AAD:        label | user_id | record_id | schema_version | revision
plaintext:  kind, data, conflicts, padded to a bucket (1, 2, 4, 8 KiB and so on, up to 1 MiB)
```

- `revision` in the AAD goes beyond the original requirement. Without it the server could return an old but valid
  version of a record, which is also a substitution.
- An encrypted manifest (`record_id → revision`) reveals rollbacks and hidden records. A device also rejects any
  revision below what it already knows. A device with no history that sees a consistently rolled-back server cannot
  detect it, because the manifest itself has no external anchor.
- Writes are compare-and-swap: the client sends the expected revision. Locking the user row makes `seq` strictly ordered,
  so a reader cannot miss a change.
- The queue is already encrypted, so sending it needs no key after locking.
- An edit leaves the queue only after the server confirms it. A repeated write after a lost response is idempotent.
- An edit made during a push is kept and re-encrypted for the next revision.
- Conflicts use a three-way merge by field. If both devices changed one field, both versions are kept and the record
  shows a visible choice. Delete against edit keeps the edit and asks the user. Deletes are tombstones inside the ciphertext.
- `updated_at` is rounded to the hour. Web Locks serialize local writes across tabs.

Known gaps:

- Restoring a database backup lowers `seq` below the devices' cursors, so devices would miss later changes.
  A restore epoch is needed before backups ship in stage 6.
- A second open tab blocks erasing the local database until that tab closes.

## 4. Finance model

All records live inside the ciphertext. Each is one sync record, so merging works per field.

- Money is an integer number of minor units, and quantities are integer thousandths. There are no floats.
  Line values round half up to a whole grosz. Every currency is treated as having two decimal places for now.
- A transaction with line items takes its amount from their sum, so the two cannot disagree on the form.
  A merge can still combine an amount and items from different devices, and the dashboard then splits the difference
  so category totals always add up to `amount_pln`.
- Category shares of a foreign-currency transaction use largest-remainder allocation, which keeps totals exact.
- Built-in categories and the main account have fixed ids, so devices never create duplicates.
  Editing one writes a record with the same id.
- Merging sets `merged_into` instead of rewriting transactions. A device that was offline during the merge can still
  send the old category id, and it resolves correctly. A merge cycle from two devices is safe but not undone automatically.
- A merchant id is an HKDF of the normalized name, keyed with the DEK. Devices agree on it offline, and because the
  server sees record ids, an unkeyed hash would let it test merchant names from a dictionary.
- Accounts and merchants in use cannot be deleted. Deleting a category means merging it.

Internal transfers, recurring payments and deduplication arrive with statement import, as they depend on it.

Statements and transfer confirmations are read on the device, like photos: a CSV through a column mapping, a PDF
through `core::pdf` (its text layer, no OCR) and `core::confirmation`. A PDF's password is typed on the page and kept
only in memory while the file is open. Only the resulting encrypted operations are synced.

## 5. Threat model

| Actor or event | What they get | Protection | Residual risk |
|---|---|---|---|
| Database or backup leak | ciphertext, sizes, hourly times, each account's creation and last use to the minute | E2EE, padding | record count, activity rhythm, when an account was last used |
| VPS admin, read-only | same, plus IP addresses in logs | logs without content, short retention | metadata |
| VPS root, active | can replace the frontend | CSP, SRI, reproducible build | **cannot be fully removed**, the code would get the key |
| Server rolls back or hides records | old records | `revision` in AAD, manifest | detection, not prevention |
| Network interception | nothing useful | HTTPS, HSTS | a compromised CA |
| XSS in the app | the key in memory | strict CSP with no inline scripts | `wasm-unsafe-eval` is required for WASM |
| Malicious dependency | the key in memory | Cargo.lock, minimal JS, `cargo audit` (advisories that do not apply are listed with reasons in `.cargo/audit.toml`) | trust in crate authors; no CI runs the audit yet |
| Tampered OCR model | hostile input to the parser | SHA-256 pinned in the build (stage 4) | none |
| Stolen locked phone | local ciphertext | key only in memory, idle lock | with PRF only ciphertext; without PRF weaker (section 2) |
| Stolen unlocked phone | data until the timeout | "Stay signed in" (per device, default 12 hours): the passkey is asked again after it, or on Lock; with "1 min", lock after 5 minutes idle or 1 minute hidden | with 12 hours, whoever holds the unlocked phone in that time opens the app |
| Login brute force | nothing | rate limit per client address (behind a proxy, the address the proxy appended, never one the client wrote), 128-bit phrase | none |
| Stranger registers | disk space | registration is open by the owner's choice; new accounts are limited per address (`REGISTER_PER_HOUR`, default 10), each account stores at most `QUOTA_MB` (default 256) of ciphertext, record and batch sizes are capped | many addresses together can still create many accounts |
| Another page sends requests with the user's cookie | could delete the account or sign out | SameSite=Strict cookie, and every request that changes state must be a same-origin JSON fetch (`Sec-Fetch-Site`, `Origin`, `Content-Type` checked) | none known |
| NBP sees rate requests | IP and period | request quarters, not transaction dates | NBP sees the IP |
| Loss of passkey and 12 words | total data loss | warning and written-copy check | **irreversible by design** |

Deleting an account removes it from the database at once. It leaves encrypted backups when those expire.

## 6. Receipt recognition

Pipeline: camera, preprocessing, on-device OCR, parsing, categorization, review screen, encryption and saving.
Early results on 3 real receipts. PP-OCR and Tesseract use preprocessing, `ocrs` does not.

| | PP-OCR v5 | Tesseract (`pol_best`) | `ocrs` |
|---|---|---|---|
| SUMA PLN | 3/3 | 1/3 | 3/3 |
| Date | 3/3 | 3/3 | 2/3 |
| Merchant | 3/3 | 2/3 | 1/3 |
| Whole line items | 7/8 | 1/8 | 0/8 |
| Polish letters, control words | 4/26 | 18/26 | 0/26 |

PP-OCR got every amount and quantity right. Its weakness is text: Polish letters, and `w` read as `u`.
Names must therefore go through a dictionary and be compared without diacritics. The sample is small, and the
prototype parser was tuned on these same receipts.

Findings that affect the design:

- Detect skew from text strokes, because a dark table around the receipt misleads a simple detector.
- Assemble lines from coordinates, because prices often come back as a separate column.
- A hard binarization threshold destroys faint print. Flattening the light and keeping gray levels works better.
- `quantity × price = value` and `item sum = SUMA PLN` also act as OCR error detectors.
- Item formats vary by cash register: all on one line, prices on the next line, or a product code in between.
- Text from a neighboring column can end up in a name, so line geometry must be checked.
- Photos include hands and other objects, so cropping to the receipt edges is required.
- Neither Tesseract nor ONNX Runtime is Rust, so OCR is an exception to "Rust everywhere" unless `rten` can run PP-OCR.

### How recognition runs in the app

1. The camera frame or chosen photo is drawn to a canvas no larger than 2400 px, turned into gray pixels,
   and the canvas is cleared at once.
2. `core::vision` crops to the paper, corrects the overall skew and flattens the light.
3. A Web Worker runs the PP-OCR detector through ONNX Runtime Web. `core::ocr` turns its map into line boxes,
   each with its own tilt, so lines on crumpled paper are straightened one by one.
4. The worker reads each line. Boxes are joined into rows by following each row from left to right,
   which keeps a row together in a photo taken at an angle.
5. `core::receipt` parses the rows and checks quantity × price, the items against `SUMA PLN`, and the PTU block.
   Anything that does not add up, and any merchant or date read with low confidence, is highlighted for review.
6. `core::categorize` assigns a category per item: dictionary, then shop defaults, then the user's own corrections.
7. Only the encrypted operation is saved. The pixels are dropped.

The recognizer files are pinned by SHA-256 or npm integrity hash and checked at build time. ONNX Runtime Web is
JavaScript and C++, an exception to "Rust everywhere" that was accepted when choosing PP-OCR.

Results on the 3 real receipts after tuning: total, merchant, date and NIP 3 of 3, items 8 of 8, about 1.3 seconds
per receipt in desktop Chrome. These receipts were used for tuning, so the result overstates accuracy on new ones.
The accuracy targets still need 30 to 50 receipts with a held-out part.

## 7. How the OCR decision is made

1. Split the receipts before tuning: about 2/3 for development and 1/3 held out.
   The accuracy targets count only on the held-out part, otherwise the parser just fits the sample.
2. With 30 to 50 receipts one error moves the result by 2 to 3 points, so 95% means about one error on the held-out part.
3. The winner passes the targets after post-processing. Among those that pass, choose the smaller and faster one on Android.

## 8. What only real devices can verify

PRF, home screen install, camera, safe areas and offline mode must be checked on real iPhones and Android phones.
Chrome's virtual authenticator and a desktop browser do not replace those checks.
