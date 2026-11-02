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
    await this.center(t);
    await this.page.click(q(t));
  }

  // A user scrolls an element clear of the fixed tab bar before tapping it.
  center(t) {
    return this.page.$eval(q(t), (e) => e.scrollIntoView({ block: "center" }));
  }

  async type(t, s) {
    await this.page.waitForSelector(q(t), { timeout: 10000 });
    await this.center(t);
    await this.page.click(q(t));
    await this.page.type(q(t), s);
  }

  async text(t) {
    await this.page.waitForSelector(q(t), { timeout: 10000 });
    return this.page.$eval(q(t), (e) => e.textContent);
  }

  waitText(t, want, timeout = 15000) {
    return this.page.waitForFunction(
      (sel, w) => [...document.querySelectorAll(sel)].some((e) => (e.value || e.textContent).replaceAll("\u00a0", " ").includes(w)),
      { timeout }, q(t), want,
    );
  }

  notes() {
    return this.page.$$eval(q("tx-title"), (a) => a.map((e) => e.textContent).sort());
  }

  async ops() {
    await this.click("nav-ops");
    await this.page.waitForSelector(q("add-tx"));
  }

  async waitNote(s) {
    await this.ops();
    await this.waitText("tx-title", s);
  }

  async addNote(s, amount = "12,34") {
    await this.click("nav-add");
    await this.type("f-amount", amount);
    await this.type("f-merchant", s);
    await this.click("f-save");
    await this.waitText("tx-title", s);
  }

  async openNote(s) {
    await this.waitNote(s);
    await this.page.evaluate((sel, t) => {
      [...document.querySelectorAll(sel)].find((b) => b.textContent.includes(t)).click();
    }, q("tx-row"), s);
    await this.page.waitForSelector(q("f-save"));
  }

  async setValue(t, v) {
    await this.page.waitForSelector(q(t));
    await this.page.$eval(q(t), (e) => { e.value = ""; });
    await this.page.focus(q(t));
    await this.page.keyboard.type(v);
  }

  async pick(t, label, index = 0) {
    await this.page.waitForSelector(q(t));
    const ok = await this.page.evaluate((sel, l, i) => {
      const s = document.querySelectorAll(sel)[i];
      const o = [...s.options].find((x) => x.textContent === l);
      if (!o) return false;
      s.value = o.value;
      s.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    }, q(t), label, index);
    if (!ok) throw new Error(`no option ${label} in ${t}`);
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
    await this.page.waitForSelector(q("dash-balance"), { timeout: 15000 });
    return words;
  }

  async login() {
    await this.click("login");
  }

  async lock() {
    await this.click("nav-settings");
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
  await phone.click("nav-settings");
  ok((await phone.text("prf-status")).includes(": on"), "PRF must work");
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
  await phone.waitNote(CANARY);
});

await test("a reload requires login", async () => {
  await phone.page.reload({ waitUntil: "load" });
  await phone.page.waitForSelector(q("login"));
  await phone.login();
  await phone.waitNote(CANARY);
});

let tablet;
await test("second device: phrase login, new passkey with PRF", async () => {
  tablet = await Device.open("tablet");
  await tablet.recover(words);
  await tablet.page.waitForSelector(q("need-passkey"));
  await tablet.type("add-phrase", words.join(" "));
  await tablet.click("add-passkey");
  await tablet.waitText("add-done", "Passkey added");
    await tablet.waitNote(CANARY);
  await tablet.lock();
  await tablet.login();
  await tablet.waitNote(CANARY);
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
  await phone.waitNote(CANARY);
  await phone.addNote("offline record");
  await phone.page.waitForSelector(q("pending"));
  await phone.offline(false);
  await phone.waitText("status", "Synced");
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.waitNote("offline record");
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
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.waitNote("while the server is down");
  const edit = async (d, text) => {
    await d.openNote("while the server is down");
    await d.setValue("f-note", text);
    await d.click("f-save");
    await d.page.waitForSelector(q("add-tx"));
  };
  await tablet.offline(true);
  await edit(phone, "phone version");
  await phone.waitText("status", "Synced");
  await edit(tablet, "tablet version");
  await tablet.offline(false);
  await tablet.page.waitForSelector(q("tx-conflict"), { timeout: 40000 });
  ok(await tablet.page.$(q("conflict-banner")), "the conflict is visible in the list");
  await tablet.openNote("while the server is down");
  await tablet.page.waitForSelector(q("conflict"));
  const buttons = await tablet.page.$$eval(`${q("conflict")} button`, (a) => a.map((b) => b.textContent));
  ok(buttons.includes("phone version") && buttons.includes("tablet version"), `both versions: ${buttons}`);
  await tablet.click("keep-theirs");
  await tablet.page.waitForSelector(q("add-tx"));
  await tablet.page.waitForFunction((s) => !document.querySelector(s), { timeout: 10000 }, q("tx-conflict"));
  await tablet.waitText("status", "Synced");
  await phone.page.reload({ waitUntil: "load" });
  await phone.login();
  await phone.openNote("while the server is down");
  eq(await phone.page.$eval(q("f-note"), (e) => e.value), "phone version", "the choice reached the second device");
  await phone.click("form-back");
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.openNote("while the server is down");
  eq(await tablet.page.$eval(q("f-note"), (e) => e.value), "phone version", "the choice stays");
  await tablet.click("form-back");
});

await test("a deletion reaches the second device", async () => {
  await phone.openNote("offline record");
  await phone.click("f-delete");
  await phone.page.waitForFunction((s) => ![...document.querySelectorAll(s)].some((e) => e.textContent === "offline record"), {}, q("tx-title"));
  await phone.waitText("status", "Synced");
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.ops();
  await tablet.page.waitForSelector(q("tx-row"));
  await tablet.page.waitForFunction((s) => ![...document.querySelectorAll(s)].some((e) => e.textContent === "offline record"), { timeout: 10000 }, q("tx-title"));
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
  await noPrf.click("nav-settings");
  ok((await noPrf.text("prf-status")).includes(": off"), "status without PRF");
    await noPrf.addNote("no prf");
  await noPrf.waitText("status", "Synced");
  await noPrf.lock();
  await noPrf.login();
  await noPrf.waitNote("no prf");
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
  await d.waitNote("no prf");
  await d.lock();
  await d.login();
  await d.waitNote("no prf");
  ok(!(await d.page.$(q("no-prf-note"))), "the phrase is no longer needed");
  await d.close();
});

await test("account deletion erases data on the server and the device", async () => {
  const before = +sql("SELECT count(*) FROM users", dbName);
  await noPrf.click("nav-settings");
  await noPrf.click("delete-account");
  eq(+sql("SELECT count(*) FROM users", dbName), before, "nothing is deleted before confirming");
  await noPrf.click("delete-confirm");
  await noPrf.page.waitForSelector(q("go-register"));
  eq(+sql("SELECT count(*) FROM users", dbName), before - 1, "users");
  const names = await noPrf.page.evaluate(async () => (await indexedDB.databases()).map((d) => d.name));
  ok(!names.some((n) => n.startsWith("mm-") && n !== "mm-device"), `local databases: ${names}`);
});

let fin;
const money = (d, t) => d.page.$eval(q(t), (e) => e.textContent.replaceAll("\u00a0", " "));

await test("dashboard totals are exact, income and expenses are separate", async () => {
  fin = await Device.open("finance");
  await fin.register();
  ok(await fin.page.$(q("empty")), "an empty account says so");
  await fin.addNote("Biedronka", "0,07");
  await fin.addNote("Lidl", "1,10");
  await fin.addNote("Żabka", "19,99");
  await fin.click("nav-add");
  await fin.click("kind-income");
  await fin.type("f-amount", "5 000,00");
  await fin.type("f-note", "Salary");
  await fin.click("f-save");
  await fin.page.waitForSelector(q("add-tx"));
  await fin.click("nav-dashboard");
  await fin.waitText("dash-expense", "21,16 zł");
  eq(await money(fin, "dash-income"), "5 000,00 zł", "income");
  eq(await money(fin, "dash-balance"), "4 978,84 zł", "balance");
});

await test("categories count per item, and a category opens its items", async () => {
  await fin.click("nav-add");
  await fin.type("f-merchant", "Rossmann");
  await fin.click("add-item");
  await fin.click("add-item");
  await fin.page.waitForFunction((s) => document.querySelectorAll(s).length === 2, {}, q("item-name"));
  const name = await fin.page.$$(q("item-name"));
  const qty = await fin.page.$$(q("item-qty"));
  const price = await fin.page.$$(q("item-price"));
  const discount = await fin.page.$$(q("item-discount"));
  const field = async (el, text, replace) => {
    await el.evaluate((e) => e.scrollIntoView({ block: "center" }));
    await el.click({ clickCount: replace ? 3 : 1 });
    await el.type(text);
  };
  await field(name[0], "Chleb");
  await field(qty[0], "0,532", true);
  await field(price[0], "5,99");
  await fin.pick("item-category", "Food", 0);
  await field(name[1], "Szampon");
  await field(qty[1], "3", true);
  await field(price[1], "8,00");
  await field(discount[1], "4,00");
  await fin.pick("item-category", "Household", 1);
  await fin.waitText("f-sum", "23,19");
  await fin.click("f-save");
  await fin.waitText("tx-title", "Rossmann");
  await fin.click("nav-dashboard");
  await fin.waitText("dash-expense", "44,35 zł");
  const rows = await fin.page.$$eval(q("cat-row"), (a) => a.map((e) => e.textContent.replaceAll("\u00a0", " ")));
  ok(rows.some((r) => r.includes("Household") && r.includes("20,00 zł")), `Household: ${rows}`);
  ok(rows.some((r) => r.includes("Food") && r.includes("3,19 zł")), `Food: ${rows}`);
  await fin.page.evaluate((sel) => {
    [...document.querySelectorAll(sel)].find((b) => b.textContent.includes("Food")).click();
  }, q("cat-row"));
  await fin.waitText("line-row", "Chleb");
  eq((await fin.page.$$(q("line-row"))).length, 1, "only the matching item");
});

await test("bad input stays on the form and changes nothing", async () => {
  await fin.click("nav-add");
  await fin.type("f-amount", "12,345");
  await fin.click("f-save");
  ok((await fin.text("form-error")).includes("amount"), "amount error");
  await fin.setValue("f-amount", "10");
  await fin.setValue("f-nip", "1234563217");
  await fin.click("f-save");
  ok((await fin.text("form-error")).includes("NIP"), "NIP error");
  await fin.setValue("f-nip", "1234563218");
  await fin.setValue("f-currency", "EUR");
  await fin.click("f-save");
  ok((await fin.text("form-error")).includes("PLN"), "a foreign amount needs PLN");
  await fin.setValue("f-pln", "43,12");
  await fin.setValue("f-merchant", "Ryanair");
  await fin.click("f-save");
  await fin.waitText("tx-amount", "10,00");
  ok((await fin.page.$$eval(q("tx-amount"), (a) => a.map((e) => e.textContent))).some((t) => t.includes("EUR")), "original currency shown");
  await fin.click("nav-dashboard");
  await fin.waitText("dash-expense", "87,47 zł");
});

await test("account filter and period change the numbers", async () => {
  await fin.click("nav-settings");
  await fin.click("manage-accounts");
  await fin.type("new-account", "Savings");
  await fin.click("add-account");
  await fin.waitText("account-name", "Savings");
  await fin.click("nav-add");
  await fin.type("f-amount", "100");
  await fin.pick("f-account", "Savings");
  await fin.click("f-save");
  await fin.page.waitForSelector(q("add-tx"));
  await fin.click("nav-dashboard");
  await fin.waitText("dash-expense", "187,47 zł");
  await fin.page.$eval("details.filters", (e) => { e.open = true; });
  await fin.pick("filter-account", "Savings");
  await fin.waitText("dash-expense", "100,00 zł");
  await fin.pick("filter-account", "All accounts");
  await fin.click("month-prev");
  await fin.waitText("dash-expense", "0,00 zł");
  await fin.click("month-next");
  await fin.waitText("dash-expense", "187,47 zł");
});

await test("an account with operations cannot be deleted", async () => {
  await fin.click("nav-settings");
  await fin.click("manage-accounts");
  await fin.page.waitForSelector(q("account-delete"));
  await fin.page.evaluate((item, del) => {
    [...document.querySelectorAll(item)].find((x) => x.querySelector("input").value === "Savings").querySelector(del).click();
  }, q("account-item"), q("account-delete"));
  ok((await fin.text("error")).includes("Move them"), "blocked");
  await fin.waitText("account-name", "Savings");
});

await test("merging a category moves its amounts", async () => {
  await fin.click("nav-dashboard");
  await fin.click("nav-settings");
  await fin.click("manage-categories");
  await fin.page.waitForSelector(q("category-item"));
  const at = await fin.page.$$eval(q("category-name"), (a) => a.findIndex((e) => e.value === "Food"));
  await fin.pick("merge-target", "Household", at);
  await (await fin.page.$$(q("merge")))[at].evaluate((e) => e.click());
  await fin.page.waitForFunction((s) => ![...document.querySelectorAll(s)].some((e) => e.value === "Food"), {}, q("category-name"));
  await fin.click("nav-dashboard");
  await fin.page.waitForFunction((s) => [...document.querySelectorAll(s)].some((e) => e.textContent.includes("Household") && e.textContent.includes("23,19")), {}, q("cat-row"));
  const rows = await fin.page.$$eval(q("cat-row"), (a) => a.map((e) => e.textContent));
  ok(!rows.some((r) => r.includes("Food")), `merged away: ${rows}`);
});

await test("renaming a merchant updates its operations and top list", async () => {
  await fin.click("nav-settings");
  await fin.click("manage-merchants");
  await fin.page.waitForSelector(q("merchant-item"));
  const at = await fin.page.$$eval(q("merchant-name"), (a) => a.findIndex((e) => e.value === "Lidl"));
  const input = (await fin.page.$$(q("merchant-name")))[at];
  await input.evaluate((e) => e.scrollIntoView({ block: "center" }));
  await input.click({ clickCount: 3 });
  await input.type("Lidl Polska");
  await (await fin.page.$$(q("merchant-save")))[at].evaluate((e) => e.click());
  await fin.waitText("merchant-name", "Lidl Polska");
  await fin.waitNote("Lidl Polska");
  await fin.click("nav-dashboard");
  await fin.page.waitForSelector(q("top-merchant"));
  await fin.click("nav-settings");
  await fin.click("manage-merchants");
  await fin.page.waitForSelector(q("merchant-item"));
  const lidl = await fin.page.$$eval(q("merchant-name"), (a) => a.findIndex((e) => e.value === "Lidl Polska"));
  await (await fin.page.$$(q("merchant-delete")))[lidl].evaluate((e) => e.click());
  ok((await fin.text("error")).includes("operations"), "a merchant in use is kept");
});

await test("the trend chart shows six months and a table of exact values", async () => {
  await fin.click("nav-dashboard");
  await fin.page.waitForSelector(q("trend"));
  eq(await fin.page.$$eval(`${q("trend")} g`, (a) => a.length), 6, "months");
  eq(await fin.page.$$eval(`${q("trend-table")} tbody tr`, (a) => a.length), 6, "table rows");
  const first = await fin.page.$eval(`${q("trend-table")} tbody tr`, (e) => e.textContent.replaceAll("\u00a0", " "));
  ok(first.includes("5 000,00") && first.includes("187,47"), `current month: ${first}`);
});

await test("CSV and JSON export contain the data, and only the device makes them", async () => {
  await fin.click("nav-settings");
  await fin.click("go-export");
  await fin.page.evaluate(() => {
    window.__blobs = [];
    const real = URL.createObjectURL.bind(URL);
    URL.createObjectURL = (b) => { b.text().then((t) => window.__blobs.push(t)); return real(b); };
  });
  const sentBefore = fin.sent.length;
  await fin.click("export-csv");
  await fin.click("export-json");
  await fin.page.waitForFunction(() => window.__blobs.length === 2);
  const [csv, json] = await fin.page.evaluate(() => window.__blobs);
  ok(csv.startsWith("type,date,time,merchant") && csv.includes("Chleb,0.532,pcs,5.99,0.00,3.19"), `csv: ${csv.slice(0, 300)}`);
  ok(csv.includes(",10.00,EUR,43.12,"), "foreign amounts in the CSV");
  const parsed = JSON.parse(json);
  eq(parsed.format, "mm-export", "format");
  eq(parsed.transactions.length, 7, "all operations");
  eq(fin.sent.length, sentBefore, "export sends nothing to the server");
});

await test("finance data is encrypted on the server", async () => {
  const sent = fin.sent.join("\n");
  for (const secret of ["Rossmann", "Chleb", "Ryanair", "Salary", "Biedronka"]) {
    ok(!sent.includes(secret), `${secret} in a request`);
  }
  const db = sql("SELECT string_agg((t)::text, E'\\n') FROM records t", dbName);
  for (const secret of ["Rossmann", "Chleb", "Ryanair"]) {
    ok(!db.includes(Buffer.from(secret).toString("hex")), `${secret} in the database`);
  }
  const local = await fin.dumpLocal();
  ok(!local.includes("Rossmann") && !local.includes("Chleb"), "plaintext in IndexedDB");
});

await test("the layout fits 320 px with nothing cut off", async () => {
  await fin.page.setViewport({ width: 320, height: 640, deviceScaleFactor: 2 });
  await fin.click("nav-dashboard");
  await fin.page.waitForSelector(q("trend"));
  for (const screen of ["nav-dashboard", "nav-ops", "nav-add", "nav-settings"]) {
    await fin.click(screen);
    await sleep(100);
    const over = await fin.page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    eq(over, 0, `horizontal overflow on ${screen}`);
  }
  await fin.click("nav-dashboard");
  await fin.page.waitForSelector(q("trend"));
  await fin.page.screenshot({ path: process.env.SHOT ?? "/tmp/mm-dashboard.png", fullPage: true });
  await fin.page.setViewport({ width: 390, height: 844, deviceScaleFactor: 2 });
});

await test("signing out erases the local copy, server data stays", async () => {
  await tablet.page.reload({ waitUntil: "load" });
  await tablet.login();
  await tablet.page.waitForSelector(q("nav-settings"));
  await tablet.click("nav-settings");
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
  const errors = [...phone.errors, ...tablet.errors, ...noPrf.errors, ...fin.errors]
    .filter((e) => !/Failed to load resource|ERR_INTERNET_DISCONNECTED|ERR_CONNECTION_REFUSED|net::/.test(e));
  eq(errors, [], "console errors");
});

await browser.close();
server.kill();
sql(`DROP DATABASE ${dbName} WITH (FORCE)`);
const failed = results.filter((r) => !r[0]).length;
console.log(`\n${results.length - failed} of ${results.length} passed`);
process.exit(failed ? 1 : 0);
