// End-to-end tests: real server, PostgreSQL, Chrome and a virtual WebAuthn authenticator.
// Run: docker compose -f compose.dev.yml up -d && web/build.sh && cargo build -p mm-server && node e2e/run.mjs
// A virtual authenticator does not replace testing on real iPhones and Android phones.

import { spawn, execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";
import puppeteer from "puppeteer-core";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const PORT = 18080;
const ORIGIN = `http://localhost:${PORT}`;
const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const CANARY = "CANARY-Żabka-1234,56";
const dbName = `e2e_${randomBytes(6).toString("hex")}`;
const dbUrl = `postgres://mm:mm-dev@127.0.0.1:55432/${dbName}`;
const bin = join(root, "target/debug/mm-server");
const env = { ...process.env, DATABASE_URL: dbUrl, RP_ID: "localhost", ORIGIN, STATIC_DIR: join(root, "web/dist"),
  LISTEN: `127.0.0.1:${PORT}`, AUTH_PER_MINUTE: "600", RUST_LOG: "warn" };

const sql = (q, db = "mm") => execFileSync("docker", ["exec", "mm-db-1", "psql", "-U", "mm", "-d", db, "-At", "-c", q]).toString();
sql(`CREATE DATABASE ${dbName}`);

let server;
const startServer = async () => {
  server = spawn(bin, [], { env, stdio: ["ignore", "inherit", "inherit"] });
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${ORIGIN}/index.html`); return; } catch { await new Promise((r) => setTimeout(r, 100)); }
  }
  throw new Error("server did not start");
};
const invite = () => execFileSync(bin, ["invite"], { env }).toString().trim();

await startServer();
const browser = await puppeteer.launch({ executablePath: CHROME, headless: true, args: ["--no-sandbox"] });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const q = (t) => `[data-test="${t}"]`;

class Device {
  sent = [];
  errors = [];

  static async open(name, { prf = true } = {}) {
    const d = new Device();
    d.name = name;
    d.ctx = await browser.createBrowserContext();
    d.page = await d.ctx.newPage();
    d.page.on("console", (m) => { if (m.type() === "error") d.errors.push(m.text()); });
    d.page.on("pageerror", (e) => { d.errors.push(String(e)); console.log(`     [${name}] ${String(e.stack ?? e).slice(0, 900)}`); });
    d.page.on("request", (r) => {
      if (r.url().includes("/api/")) d.sent.push(`${r.method()} ${r.url()}\n${r.postData() ?? ""}`);
    });
    d.cdp = await d.page.createCDPSession();
    await d.cdp.send("WebAuthn.enable");
    const { authenticatorId } = await d.cdp.send("WebAuthn.addVirtualAuthenticator", { options: {
      protocol: "ctap2", ctap2Version: "ctap2_1", transport: "internal", hasResidentKey: true,
      hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true, hasPrf: prf,
    } });
    d.auth = authenticatorId;
    await d.page.goto(ORIGIN, { waitUntil: "load" });
    await d.page.waitForSelector(q("app"));
    return d;
  }

  async click(t) {
    await this.page.waitForSelector(`${q(t)}:not([disabled])`, { timeout: 10000 });
    await this.page.click(q(t));
  }

  async type(t, s) {
    await this.page.waitForSelector(q(t), { timeout: 10000 });
    await this.page.click(q(t));
    await this.page.type(q(t), s);
  }

  async text(t) {
    await this.page.waitForSelector(q(t), { timeout: 10000 });
    return this.page.$eval(q(t), (e) => e.textContent);
  }

  waitText(t, want, timeout = 15000) {
    return this.page.waitForFunction(
      (sel, w) => [...document.querySelectorAll(sel)].some((e) => (e.value ?? e.textContent).includes(w)),
      { timeout }, q(t), want,
    );
  }

  notes() {
    return this.page.$$eval(q("note-text"), (a) => a.map((e) => e.value).sort());
  }

  async addNote(s) {
    await this.type("draft", s);
    await this.click("add");
    await this.waitText("note-text", s);
  }

  async register() {
    await this.click("go-register");
    await this.type("invite", invite());
    await this.type("label", this.name);
    await this.click("invite-next");
    await this.click("accept");
    await this.click("warning-next");
    await this.page.waitForSelector(q("words"));
    const words = await this.page.$$eval(`${q("words")} li`, (a) => a.map((e) => e.textContent));
    await this.click("phrase-next");
    await this.page.waitForSelector(q("confirm-word"));
    const fields = await this.page.$$(q("confirm-word"));
    for (const f of fields) {
      const pos = await f.evaluate((e) => +e.dataset.pos);
      await f.type(words[pos]);
    }
    await this.click("confirm-next");
    await this.click("create-passkey");
    await this.page.waitForSelector(q("draft"), { timeout: 15000 });
    return words;
  }

  async login() {
    await this.click("login");
  }

  async lock() {
    await this.click("go-settings");
    await this.click("lock");
    await this.page.waitForSelector(q("login"));
  }

  async recover(words) {
    await this.click("go-login");
    await this.click("go-recover");
    await this.type("phrase-input", words.join(" "));
    await this.click("phrase-submit");
  }

  // Cuts the network at browser level, service worker included.
  async offline(on) {
    const conditions = { offline: on, latency: 0, downloadThroughput: -1, uploadThroughput: -1 };
    await this.cdp.send("Network.enable");
    await this.cdp.send("Network.emulateNetworkConditions", conditions);
  }

  async dumpLocal() {
    return this.page.evaluate(async () => {
      const out = [];
      for (const { name } of await indexedDB.databases()) {
        const db = await new Promise((ok, no) => { const r = indexedDB.open(name); r.onsuccess = () => ok(r.result); r.onerror = no; });
        for (const s of db.objectStoreNames) {
          const vals = await new Promise((ok) => { const r = db.transaction(s).objectStore(s).getAll(); r.onsuccess = () => ok(r.result); });
          out.push(...vals.map((v) => String(v)));
        }
        db.close();
      }
      return out.join("\n") + localStorage.length + sessionStorage.length;
    });
  }

  async close() {
    await this.ctx.close();
  }
}

const results = [];
const test = async (name, f) => {
  const t = Date.now();
  try {
    await f();
    results.push([true, name]);
    console.log(`ok   ${name} (${Date.now() - t} ms)`);
  } catch (e) {
    results.push([false, name]);
    console.log(`FAIL ${name}\n     ${e.stack}`);
  }
};
const eq = (a, b, m) => { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${m}: ${JSON.stringify(a)} != ${JSON.stringify(b)}`); };
const ok = (v, m) => { if (!v) throw new Error(m); };

let words, phone;

await test("registration with PRF: phrase, confirmation, passkey", async () => {
  phone = await Device.open("phone");
  words = await phone.register();
  eq(words.length, 12, "words");
  await phone.click("go-settings");
  ok((await phone.text("prf-status")).includes(": on"), "PRF must work");
  await phone.click("go-home");
});

await test("wrong words fail the confirmation", async () => {
  const d = await Device.open("wrong");
  await d.click("go-register");
  await d.type("invite", invite());
  await d.click("invite-next");
  await d.click("accept");
  await d.click("warning-next");
  await d.click("phrase-next");
  await d.page.waitForSelector(q("confirm-word"));
  for (const f of await d.page.$$(q("confirm-word"))) await f.type("wrong");
  await d.click("confirm-next");
  ok((await d.text("error")).includes("do not match"), "error");
  ok(!(await d.page.$(q("create-passkey"))), "must not get past this step");
  await d.close();
});

await test("records are encrypted: no plaintext on the server or in browser storage", async () => {
  await phone.addNote(CANARY);
  await phone.waitText("status", "Synced");
  await phone.page.waitForFunction((s) => !document.querySelector(s), {}, q("pending"));
  const sent = phone.sent.join("\n");
  ok(sent.includes("/api/sync/push"), "the record must reach the server");
  ok(!sent.includes(CANARY) && !sent.includes("1234,56"), "plaintext in a request");
  ok(!sent.includes('"prf"') && !sent.includes("results"), "the PRF output must not leave the device");
  ok(!sent.includes(words[0] + " " + words[1]), "phrase in a request");
  const local = await phone.dumpLocal();
  ok(local.length > 100, "a local copy exists");
  ok(!local.includes(CANARY) && !local.includes("Żabka"), "plaintext in IndexedDB");
  const db = sql("SELECT string_agg((t)::text, E'\\n') FROM records t", dbName) + sql("SELECT string_agg((t)::text, E'\\n') FROM users t", dbName);
  const hex = Buffer.from(CANARY).toString("hex");
  ok(db.length > 100 && !db.includes(CANARY) && !db.includes(hex), "plaintext in the database");
});

await test("locking clears the data, the passkey opens it again", async () => {
  await phone.lock();
  eq(await phone.notes(), [], "no records on screen after locking");
  await phone.login();
  await phone.waitText("note-text", CANARY);
});

await test("a reload requires login", async () => {
  await phone.page.reload({ waitUntil: "load" });
  await phone.page.waitForSelector(q("login"));
  await phone.login();
  await phone.waitText("note-text", CANARY);
});

let tablet;
await test("second device: phrase login, new passkey with PRF", async () => {
  tablet = await Device.open("tablet");
  await tablet.recover(words);
  await tablet.page.waitForSelector(q("need-passkey"));
  await tablet.type("add-phrase", words.join(" "));
  await tablet.click("add-passkey");
  await tablet.waitText("add-done", "Passkey added");
  await tablet.click("go-home");
  await tablet.waitText("note-text", CANARY);
  await tablet.lock();
  await tablet.login();
  await tablet.waitText("note-text", CANARY);
});

await test("a wrong phrase does not open the account", async () => {
  const d = await Device.open("thief");
  await d.click("go-login");
  await d.click("go-recover");
  await d.type("phrase-input", "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
  await d.click("phrase-submit");
  ok((await d.text("error")).includes("does not match"), "error");
  await d.close();
});

await test("a passkey cannot be added without the right phrase", async () => {
  const d = await Device.open("session-thief");
  await d.recover(words);
  await d.page.waitForSelector(q("need-passkey"));
  await d.type("add-phrase", "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
  await d.click("add-passkey");
  ok((await d.text("error")).includes("does not match"), "error");
  await d.close();
});

await test("offline: the app opens, the passkey unlocks, edits wait for the network", async () => {
  await phone.page.waitForFunction(() => navigator.serviceWorker.controller !== null || true);
  await phone.page.evaluate(() => navigator.serviceWorker.ready);
  await phone.page.reload({ waitUntil: "load" }); // the service worker controls the page now
  await phone.page.waitForSelector(q("login"));
  ok(await phone.page.evaluate(() => !!navigator.serviceWorker.controller), "service worker is active");
  await phone.offline(true);
  ok(await phone.page.evaluate(() => !navigator.onLine), "browser offline before reload");
  await phone.page.reload({ waitUntil: "load" });
  await phone.page.waitForSelector(q("login"));
  await phone.offline(false); // a reload resets emulation, so apply it again
  await phone.offline(true);
  ok(await phone.page.evaluate(() => !navigator.onLine), "browser offline after reload");
  await phone.login();
  await phone.waitText("note-text", CANARY);
  await phone.addNote("offline record");
  await phone.page.waitForSelector(q("pending"));
  await phone.offline(false);
  await phone.waitText("status", "Synced");
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.waitText("note-text", "offline record");
});

await test("server crash: edits are kept and sent after recovery", async () => {
  server.kill("SIGKILL");
  await sleep(300);
  try {
    await phone.addNote("while the server is down");
    await phone.waitText("status", "changes saved");
    await phone.page.waitForSelector(q("pending"));
  } finally {
    await startServer();
  }
  await phone.page.reload({ waitUntil: "load" });
  await phone.login();
  await phone.waitText("status", "Synced");
  await phone.page.waitForFunction((s) => !document.querySelector(s), {}, q("pending"));
});

await test("conflict: both versions shown, the choice reaches both devices", async () => {
  await tablet.click("go-settings");
  await tablet.click("go-home");
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.waitText("note-text", "while the server is down");
  const edit = async (d, text) => {
    const li = (await d.page.$$(q("note"))).at(-1);
    const input = await li.$(q("note-text"));
    await input.click({ clickCount: 3 });
    await input.type(text);
    await (await li.$(q("note-save"))).click();
  };
  // Both devices pick the same record: the sort order is the same.
  await tablet.offline(true);
  await edit(phone, "phone version");
  await phone.waitText("status", "Synced");
  await edit(tablet, "tablet version");
  await tablet.offline(false);
  await tablet.page.waitForSelector(q("conflict"), { timeout: 40000 });
  const buttons = await tablet.page.$$eval(`${q("conflict")} button`, (a) => a.map((b) => b.textContent));
  ok(buttons.includes("phone version") && buttons.includes("tablet version"), `both versions: ${buttons}`);
  await tablet.click("keep-theirs");
  await tablet.page.waitForFunction((s) => !document.querySelector(s), { timeout: 10000 }, q("conflict"));
  await tablet.waitText("status", "Synced");
  await phone.page.reload({ waitUntil: "load" });
  await phone.login();
  await phone.waitText("note-text", "phone version");
  ok(!(await phone.notes()).includes("tablet version"), "the choice reached the second device");
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.waitText("note-text", "phone version");
});

await test("a deletion reaches the second device", async () => {
  const li = (await phone.page.$$(q("note"))).find(() => true);
  const text = await li.$eval(q("note-text"), (e) => e.value);
  await (await li.$(q("note-delete"))).click();
  await phone.page.waitForFunction((s, t) => ![...document.querySelectorAll(s)].some((e) => e.value === t), {}, q("note-text"), text);
  await phone.waitText("status", "Synced");
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.page.waitForSelector(q("note"));
  await tablet.page.waitForFunction((s, t) => ![...document.querySelectorAll(s)].some((e) => e.value === t), { timeout: 10000 }, q("note-text"), text);
});

await test("idle timeout locks the app and clears the data", async () => {
  // Move the page clock 6 minutes ahead, so the next timer tick locks.
  await phone.page.evaluate(() => { const real = Date.now; Date.now = () => real.call(Date) + 6 * 60 * 1000; });
  await phone.page.waitForSelector(q("login"), { timeout: 40000 });
  eq(await phone.notes(), [], "data removed from the screen");
});

let noPrf, noPrfWords;
await test("device without PRF: registration and passkey login work", async () => {
  noPrf = await Device.open("no-prf", { prf: false });
  noPrfWords = await noPrf.register();
  await noPrf.click("go-settings");
  ok((await noPrf.text("prf-status")).includes(": off"), "status without PRF");
  await noPrf.click("go-home");
  await noPrf.addNote("no prf");
  await noPrf.waitText("status", "Synced");
  await noPrf.lock();
  await noPrf.login();
  await noPrf.waitText("note-text", "no prf");
});

await test("new device without PRF: 12 words once, then passkey only", async () => {
  const { credentials } = await noPrf.cdp.send("WebAuthn.getCredentials", { authenticatorId: noPrf.auth });
  const d = await Device.open("no-prf-2", { prf: false });
  await d.cdp.send("WebAuthn.addCredential", { authenticatorId: d.auth, credential: credentials[0] });
  await d.click("go-login");
  await d.login();
  await d.page.waitForSelector(q("no-prf-note"));
  await d.type("phrase-input", noPrfWords.join(" "));
  await d.click("phrase-submit");
  await d.waitText("note-text", "no prf");
  await d.lock();
  await d.login();
  await d.waitText("note-text", "no prf");
  ok(!(await d.page.$(q("no-prf-note"))), "the phrase is no longer needed");
  await d.close();
});

await test("account deletion erases data on the server and the device", async () => {
  const before = +sql("SELECT count(*) FROM users", dbName);
  await noPrf.click("go-settings");
  await noPrf.click("delete-account");
  eq(+sql("SELECT count(*) FROM users", dbName), before, "nothing is deleted before confirming");
  await noPrf.click("delete-confirm");
  await noPrf.page.waitForSelector(q("go-register"));
  eq(+sql("SELECT count(*) FROM users", dbName), before - 1, "users");
  const names = await noPrf.page.evaluate(async () => (await indexedDB.databases()).map((d) => d.name));
  ok(!names.some((n) => n.startsWith("mm-") && n !== "mm-device"), `local databases: ${names}`);
});

await test("signing out erases the local copy, server data stays", async () => {
  await tablet.page.waitForSelector(q("go-settings"));
  await tablet.click("go-settings");
  await tablet.click("forget-device");
  await tablet.page.waitForSelector(q("go-register"));
  const names = await tablet.page.evaluate(async () => (await indexedDB.databases()).map((d) => d.name));
  ok(!names.some((n) => n.startsWith("mm-") && n !== "mm-device"), `local databases: ${names}`);
  ok(+sql("SELECT count(*) FROM records", dbName) > 0, "records are still on the server");
});

await test("strict CSP and SRI: no violations, no inline scripts", async () => {
  const r = await fetch(`${ORIGIN}/`);
  const csp = r.headers.get("content-security-policy");
  ok(csp.includes("script-src 'self' 'wasm-unsafe-eval'") && !csp.includes("unsafe-inline"), "CSP");
  const html = await r.text();
  ok(!/<script(?![^>]*src=)/.test(html), "inline script");
  ok(!html.includes("__") && html.includes('integrity="sha384-'), "SRI");
  const errors = [...phone.errors, ...tablet.errors, ...noPrf.errors]
    .filter((e) => !/Failed to load resource|ERR_INTERNET_DISCONNECTED|ERR_CONNECTION_REFUSED|net::/.test(e));
  eq(errors, [], "console errors");
});

await browser.close();
server.kill();
sql(`DROP DATABASE ${dbName} WITH (FORCE)`);
const failed = results.filter((r) => !r[0]).length;
console.log(`\n${results.length - failed} of ${results.length} passed`);
process.exit(failed ? 1 : 0);
