// Makes the screenshots on the welcome page from made-up data: a fresh account, a synthetic bank statement,
// then Home in the dark theme and Stats in the light one.
// node e2e/shots.mjs   (needs the test DB, web/dist and target/debug/mm-server, like run.mjs)
// Writes web/static/shot-dark.webp and web/static/shot-light.webp.

import { spawn, execFileSync } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";
import puppeteer from "puppeteer-core";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const PORT = 18082;
const ORIGIN = `http://localhost:${PORT}`;
const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const dbName = `shots_${randomBytes(6).toString("hex")}`;
const bin = join(root, "target/debug/mm-server");
const env = { ...process.env, DATABASE_URL: `postgres://mm:mm-dev@127.0.0.1:55432/${dbName}`, RP_ID: "localhost", ORIGIN,
  STATIC_DIR: join(root, "web/dist"), LISTEN: `127.0.0.1:${PORT}`, AUTH_PER_MINUTE: "600", RUST_LOG: "warn" };
execFileSync("docker", ["exec", "mm-db-1", "psql", "-U", "mm", "-d", "mm", "-At", "-c", `CREATE DATABASE ${dbName}`]);
const server = spawn(bin, [], { env, stdio: "ignore" });
const cleanup = () => { try { server.kill(); } catch {} };
process.on("exit", cleanup);
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => { cleanup(); process.exit(1); });
for (let i = 0; i < 100; i++) {
  try { await fetch(`${ORIGIN}/index.html`); break; } catch { await new Promise((r) => setTimeout(r, 100)); }
}

// A made-up statement: this month up to today, and the two months before it, so the trend has bars.
const today = new Date();
const day = (monthsBack, d) => {
  const x = new Date(today.getFullYear(), today.getMonth() - monthsBack, Math.min(d, monthsBack ? 28 : today.getDate()));
  return `${x.getFullYear()}-${String(x.getMonth() + 1).padStart(2, "0")}-${String(x.getDate()).padStart(2, "0")}`;
};
const month = [
  [1, "WYNAGRODZENIE salary", "6 200,00"],
  [1, "SKLEP SPOZYWCZY KWIATOWA chleb mleko", "-64,37"],
  [2, "STACJA PALIW pb95", "-210,00"],
  [3, "KAWIARNIA RATUSZ latte", "-24,00"],
  [4, "DROGERIA szampon plyn do naczyn", "-47,15"],
  [5, "SKLEP SPOZYWCZY kawa herbata", "-41,22"],
  [5, "STREAMING SUBSKRYPCJA", "-43,00"],
  [6, "APTEKA POD LIPAMI", "-38,90"],
  [7, "RESTAURACJA STARY RYNEK obiad", "-132,50"],
  [8, "KINO MUZA seans", "-58,00"],
  [9, "SKLEP SPOZYWCZY KWIATOWA pieczywo sok ", "-52,80"],
  [9, "BILET MPK miesieczny", "-110,00"],
];
const earlier = [
  [1, "WYNAGRODZENIE salary", "6 200,00"],
  [3, "SKLEP SPOZYWCZY KWIATOWA chleb", "-188,40"],
  [5, "STREAMING SUBSKRYPCJA", "-43,00"],
  [9, "STACJA PALIW diesel", "-240,00"],
  [14, "SKLEP ODZIEZOWY kurtka", "-249,99"],
  [18, "RESTAURACJA STARY RYNEK obiad", "-96,00"],
  [22, "DROGERIA proszek", "-61,30"],
  [26, "SKLEP SPOZYWCZY KWIATOWA kawa", "-142,75"],
];
const rows = [
  ...month.filter(([d]) => d <= today.getDate()).map(([d, t, a]) => [day(0, d), t, a]),
  ...[1, 2].flatMap((m) => earlier.map(([d, t, a]) => [day(m, d), t, a])),
];
const dir = mkdtempSync(join(tmpdir(), "mm-shots-"));
const csv = join(dir, "demo.csv");
writeFileSync(csv, ["#Data operacji;#Opis operacji;#Rachunek;#Kategoria;#Kwota;#Waluta;",
  ...rows.map(([d, t, a]) => `${d};"${t}";Konto;;${a};PLN;`)].join("\n") + "\n");

const q = (t) => `[data-test="${t}"]`;
const browser = await puppeteer.launch({ executablePath: CHROME, headless: true, args: ["--no-sandbox"] });
const page = await browser.newPage();
await page.setViewport({ width: 390, height: 844, deviceScaleFactor: 2 });
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
await click("go-register");
await page.type(q("invite"), execFileSync(bin, ["invite"], { env }).toString().trim());
await click("invite-next");
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

await click("nav-settings");
await click("go-import");
await (await page.waitForSelector(q("import-file"))).uploadFile(csv);
await page.waitForSelector(q("import-counts"));
await click("import-run");
await page.waitForSelector(q("import-summary"));

const settle = () => new Promise((r) => setTimeout(r, 600));
await click("nav-dashboard");
await page.waitForSelector(q("tx-row"));
await settle();
await page.screenshot({ path: join(root, "web/static/shot-dark.webp"), type: "webp", quality: 80 });

await click("nav-settings");
await click("theme-light");
await page.waitForFunction(() => document.documentElement.dataset.theme === "light");
await click("nav-stats");
await page.waitForSelector(q("pie-slice"));
await page.$eval("main", (m) => m.scrollTo(0, 0));
await settle();
await page.screenshot({ path: join(root, "web/static/shot-light.webp"), type: "webp", quality: 80 });

await browser.close();
console.log(`${rows.length} made-up operations; wrote web/static/shot-dark.webp and shot-light.webp`);
process.exit(0);
