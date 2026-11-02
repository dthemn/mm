# mm

A personal income and expense tracker for a family in Poland: a mobile web app (PWA) in Rust with end-to-end encryption.
The main flow: photograph a receipt, and the app handles the rest on the device.

> **Status: early development, not usable for real bookkeeping yet.**
> Stage 2 of 6 is done: accounts, encryption and sync work, tested in desktop Chrome with a virtual authenticator.
> Nothing has been tested on a real iPhone or Android phone. The record list is a placeholder for the
> transactions coming in stage 3. Receipts, imports and the dashboard do not exist yet.

## Principles

1. The server never receives data in the clear. The database holds only ciphertext.
2. Recognition, parsing and analytics run only on the device. Photos and PDFs are never uploaded.
3. At most 3 taps from opening the app to a saved receipt.
4. Reliability beats features: money is counted exactly, in integer grosze, and a sync failure cannot lose data.

## Layout

| Path | Purpose |
|---|---|
| `core/` | Cryptography, record format, three-way merge and sync. Builds natively and to WASM. |
| `server/` | Axum, sqlx, PostgreSQL. Stores only encrypted records and minimal metadata. |
| `web/` | Leptos CSR client, IndexedDB storage, WebAuthn PRF, service worker. |
| `e2e/` | End-to-end tests in Chrome against a real server and database. |
| `tools/ocr-bench/` | Local comparison of OCR engines on Polish receipts. |
| `docs/ARCHITECTURE.md` | Architecture, key hierarchy and threat model. |

## How accounts work

- Sign-in uses a passkey (Face ID or fingerprint) through WebAuthn.
- During registration the client creates a random 256-bit data key (DEK) and a 12-word BIP39 phrase.
- The server stores the DEK only in wrapped form: one copy under the passkey's PRF output, one under the phrase.
- The 12 words are a backup for a lost passkey, and are also needed to add a passkey.
- Each family member has a separate account and a separate key.
- **There is no password reset. Losing both the passkey and the 12 words means losing the data.**

Records are encrypted with XChaCha20-Poly1305. The user id, record id, schema version and revision are bound
as AAD, so the server cannot swap records or quietly restore an old version.

## Development

Requires Rust with the `wasm32-unknown-unknown` target, `wasm-bindgen-cli` matching the version in `Cargo.lock`,
Docker, Node.js and Chrome.

```sh
docker compose -f compose.dev.yml up -d

cargo test -p mm-core
cargo test -p mm-server

web/build.sh
cargo build -p mm-server
(cd e2e && npm install)
node e2e/run.mjs
```

To run the app locally:

```sh
export DATABASE_URL=postgres://mm:mm-dev@127.0.0.1:55432/mm
export RP_ID=localhost ORIGIN=http://localhost:8080 STATIC_DIR=web/dist
cargo run -p mm-server -- invite    # prints an invite code
cargo run -p mm-server
```

Open <http://localhost:8080>. Registration works only with an invite.

## Limits of web end-to-end encryption

A database administrator sees only ciphertext. However, whoever controls the server that delivers the frontend can
in theory replace the app's code, and that code would get the key. A web app cannot fully rule this out.
A strict CSP, SRI and a reproducible build reduce the risk. For a family server under your own control this is acceptable.

## Sample data

Real receipts and statements stay in the local `samples/` folder, which is ignored by git.
Only anonymized or synthetic data belongs in the repository.

## License

[MIT](LICENSE)
