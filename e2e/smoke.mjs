// Checks a running deployment in Chrome with a virtual passkey:
// INVITE=<code> node e2e/smoke.mjs [origin]
import puppeteer from "puppeteer-core";

const ORIGIN = process.argv[2] ?? "http://localhost:8080";
const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const q = (t) => `[data-test="${t}"]`;
const NOTE = `smoke ${Date.now()}`;

const browser = await puppeteer.launch({ executablePath: CHROME, headless: true, args: ["--no-sandbox"] });
const page = await browser.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
const cdp = await page.createCDPSession();
await cdp.send("WebAuthn.enable");
await cdp.send("WebAuthn.addVirtualAuthenticator", { options: {
  protocol: "ctap2", ctap2Version: "ctap2_1", transport: "internal", hasResidentKey: true,
  hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true, hasPrf: true,
} });
const click = async (t) => {
  await page.waitForSelector(`${q(t)}:not([disabled])`, { timeout: 10000 });
  await page.click(q(t));
};
const type = async (t, s) => {
  await page.waitForSelector(q(t), { timeout: 10000 });
  await page.type(q(t), s);
};
const hasNote = () => page.waitForFunction(
  (s, n) => [...document.querySelectorAll(s)].some((e) => e.value === n), { timeout: 15000 }, q("note-text"), NOTE,
);

try {
  await page.goto(ORIGIN, { waitUntil: "load" });
  await click("go-register");
  await type("invite", process.env.INVITE);
  await type("label", "smoke");
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
  await type("draft", NOTE);
  await click("add");
  await hasNote();
  await page.waitForFunction((s) => document.querySelector(s)?.textContent === "Synced", { timeout: 15000 }, q("status"));
  await page.reload({ waitUntil: "load" });
  await click("login");
  await hasNote();
  if (errors.length) throw new Error(errors.join("\n"));
  console.log("smoke test passed");
} finally {
  await browser.close();
}
