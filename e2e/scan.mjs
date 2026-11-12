// Runs receipt photos through the real app in Chrome and scores the review form against ground truth.
// SAMPLES=samples/receipts node e2e/scan.mjs
// Uses the same server, database and virtual passkey setup as run.mjs. Only scores are printed.

import { spawn, execFileSync } from "node:child_process";
import { readdirSync, readFileSync, existsSync } from "node:fs";
import { dirname, join, extname, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes, createHash } from "node:crypto";
import puppeteer from "puppeteer-core";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const SAMPLES = process.env.SAMPLES ?? join(root, "samples/receipts");
const PORT = 18081;
const ORIGIN = `http://localhost:${PORT}`;
const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const dbName = `scan_${randomBytes(6).toString("hex")}`;
const bin = join(root, "target/debug/mm-server");
const env = { ...process.env, DATABASE_URL: `postgres://mm:mm-dev@127.0.0.1:55432/${dbName}`, RP_ID: "localhost", ORIGIN,
  STATIC_DIR: join(root, "web/dist"), LISTEN: `127.0.0.1:${PORT}`, AUTH_PER_MINUTE: "600", REGISTER_PER_HOUR: "1000", RUST_LOG: "warn" };
const sql = (q) => execFileSync("docker", ["exec", "mm-db-1", "psql", "-U", "mm", "-d", "mm", "-At", "-c", q]).toString();
sql(`CREATE DATABASE ${dbName}`);
// The server must not hold this script's output open, and must stop when the script stops.
const server = spawn(bin, [], { env, stdio: "ignore" });
const cleanup = () => { try { server.kill(); } catch {} };
process.on("exit", cleanup);
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => { cleanup(); process.exit(1); });
for (let i = 0; i < 100; i++) {
  try { await fetch(`${ORIGIN}/index.html`); break; } catch { await new Promise((r) => setTimeout(r, 100)); }
}

const q = (t) => `[data-test="${t}"]`;
const browser = await puppeteer.launch({ executablePath: CHROME, headless: true, args: ["--no-sandbox"] });
const page = await browser.newPage();
await page.setViewport({ width: 390, height: 844, deviceScaleFactor: 2 });
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
page.on("console", (m) => { if (process.env.SHOW && m.text().startsWith("ocr ")) console.log(m.text()); });
page.on("workercreated", (w) => { if (process.env.SHOW) console.log("worker:", w.url()); });
const cdp = await page.createCDPSession();
await cdp.send("WebAuthn.enable");
await cdp.send("WebAuthn.addVirtualAuthenticator", { options: {
  protocol: "ctap2", ctap2Version: "ctap2_1", transport: "internal", hasResidentKey: true,
  hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true, hasPrf: true,
} });
const click = async (t) => {
  await page.waitForSelector(`${q(t)}:not([disabled])`, { timeout: 60000 });
  await page.$eval(q(t), (e) => e.scrollIntoView({ block: "center" }));
  await page.click(q(t));
};

await page.goto(ORIGIN, { waitUntil: "load" });
if (process.env.SHOW) await page.evaluate(() => localStorage.setItem("mmDebug", "1"));
await click("go-register");
await click("new-next");
await click("accept");
await click("warning-next");
await page.waitForSelector(q("words"));
const words = await page.$$eval(`${q("words")} li`, (a) => a.map((e) => e.textContent));
await click("phrase-next");
await page.waitForSelector(q("confirm-word"));
for (const f of await page.$$(q("confirm-word"))) await f.type(words[await f.evaluate((e) => +e.dataset.pos)]);
await click("confirm-next");
await click("create-passkey");
await page.waitForSelector(q("dash-balance"), { timeout: 20000 });

const fold = (s) => (s ?? "").normalize("NFKD").replace(/ł/g, "l").replace(/Ł/g, "L").replace(/[̀-ͯ]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();
const sim = (a, b) => {
  a = fold(a); b = fold(b);
  if (!a.length && !b.length) return 1;
  const d = Array.from({ length: a.length + 1 }, (_, i) => [i, ...Array(b.length).fill(0)]);
  for (let j = 1; j <= b.length; j++) d[0][j] = j;
  for (let i = 1; i <= a.length; i++) for (let j = 1; j <= b.length; j++)
    d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
  return 1 - d[a.length][b.length] / Math.max(a.length, b.length);
};
const grosze = (s) => {
  const m = (s ?? "").replace(/\s/g, "").match(/^(\d+)(?:,(\d{1,2}))?$/);
  return m ? +m[1] * 100 + +(m[2] ?? "0").padEnd(2, "0") : null;
};

const seen = new Set();
const files = readdirSync(SAMPLES).filter((f) => /\.(jpe?g|png|heic|webp)$/i.test(f)).sort().filter((f) => {
  const h = createHash("sha256").update(readFileSync(join(SAMPLES, f))).digest("hex");
  if (seen.has(h)) return false;
  seen.add(h);
  return true;
});
const rows = [];
for (const f of files) {
  const truthPath = join(SAMPLES, "truth", basename(f, extname(f)) + ".json");
  const truth = existsSync(truthPath) ? JSON.parse(readFileSync(truthPath, "utf8")) : null;
  await click("nav-photo");
  const input = await page.waitForSelector(q("scan-file"));
  const t0 = Date.now();
  await input.uploadFile(join(SAMPLES, f));
  const LIMIT = +(process.env.LIMIT ?? 120000);
  const ticker = setInterval(async () => {
    if (!process.env.SHOW) return;
    const st = await page.$eval(q("scan-status"), (e) => e.textContent).catch(() => "-");
    console.log(`  ${((Date.now() - t0) / 1000).toFixed(0)}s status: ${st}`);
  }, 3000);
  const outcome = await Promise.race([
    page.waitForSelector(q("f-save"), { timeout: LIMIT }).then(() => "form"),
    page.waitForSelector(q("error"), { timeout: LIMIT }).then(() => "error"),
  ]).catch(() => "timeout").finally(() => clearInterval(ticker));
  if (outcome === "timeout") {
    rows.push({ file: f, secs: LIMIT / 1000, failed: "timeout" });
    break;
  }
  const secs = (Date.now() - t0) / 1000;
  if (outcome !== "form") {
    rows.push({ file: f, secs, failed: await page.$eval(q("error"), (e) => e.textContent) });
    continue;
  }
  const form = await page.evaluate(() => {
    const sel = (t) => `[data-test="${t}"]`;
    return {
    merchant: document.querySelector(sel("f-merchant")).value,
    date: document.querySelector(sel("f-date")).value,
    total: document.querySelector(sel("f-amount")).value,
    nip: document.querySelector(sel("f-nip")).value,
    items: [...document.querySelectorAll(sel("item"))].map((li) => ({
      name: li.querySelector(sel("item-name")).value,
      qty: li.querySelector(sel("item-qty")).value,
      price: li.querySelector(sel("item-price")).value,
      category: (() => { const s = li.querySelector(sel("item-category")); return s.options[s.selectedIndex]?.text; })(),
    })),
    doubts: document.querySelectorAll(".doubt").length,
    mismatch: !!document.querySelector(sel("mismatch")),
    };
  });
  const r = { file: f, secs, doubts: form.doubts, mismatch: form.mismatch, items: form.items.length };
  if (truth) {
    r.total = grosze(form.total) === truth.total;
    r.date = form.date === truth.date;
    r.merchant = sim(form.merchant, truth.merchant) >= 0.85;
    r.nip = form.nip === truth.nip;
    const pool = [...form.items];
    let okItems = 0;
    for (const ti of truth.items) {
      const best = pool.map((x, i) => [sim(x.name, ti.name), i]).sort((a, b) => b[0] - a[0])[0];
      if (!best || best[0] < 0.8) continue;
      const x = pool[best[1]];
      if (x.qty.replace(".", ",") === ti.qty && grosze(x.price) === ti.price) {
        okItems++;
        pool.splice(best[1], 1);
      }
    }
    r.itemsOk = okItems;
    r.itemsTruth = truth.items.length;
  }
  rows.push(r);
  if (process.env.SHOW) console.log("FORM", JSON.stringify(form));
  await click("form-back");
}

const scored = rows.filter((r) => r.itemsTruth !== undefined);
const pct = (k) => scored.length ? Math.round((100 * scored.filter((r) => r[k]).length) / scored.length) : null;
console.table(rows.map(({ file, ...r }) => ({ file: file.slice(-12), ...r })));
if (scored.length) {
  const it = scored.reduce((a, r) => [a[0] + r.itemsOk, a[1] + r.itemsTruth], [0, 0]);
  console.log(`receipts ${scored.length}: total ${pct("total")}%, merchant ${pct("merchant")}%, date ${pct("date")}%, nip ${pct("nip")}%, items ${Math.round((100 * it[0]) / it[1])}%`);
}
console.log("page errors:", errors.length ? errors : "none");
await browser.close();
server.kill();
sql(`DROP DATABASE ${dbName} WITH (FORCE)`);
