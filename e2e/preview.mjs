// Screenshots of the main screens at one window size, from made-up data, to look at a layout by eye.
// WIDTH=1280 HEIGHT=800 OUT=/some/dir node e2e/preview.mjs   (defaults: 1280 × 800, the current directory)
// Needs the test DB, web/dist and target/debug/mm-server, like run.mjs.

import { join } from "node:path";
import { demo, settle, q, ORIGIN } from "./demo.mjs";

const width = +(process.env.WIDTH ?? 1280);
const height = +(process.env.HEIGHT ?? 800);
const out = process.env.OUT ?? ".";
const { browser, page, click } = await demo({ width, height });
const shot = async (name) => {
  await settle();
  await page.screenshot({ path: join(out, `${width}-${name}.png`) });
};

// The front page, in a separate window that has no account.
const visitor = await (await browser.createBrowserContext()).newPage();
await visitor.setViewport({ width, height });
await visitor.goto(ORIGIN, { waitUntil: "load" });
await visitor.waitForSelector(q("landing"));
await settle();
await visitor.screenshot({ path: join(out, `${width}-landing.png`) });
await visitor.screenshot({ path: join(out, `${width}-landing-full.png`), fullPage: true });
await visitor.$eval("main", (m) => m.scrollTo(0, m.scrollHeight));
await settle();
await visitor.screenshot({ path: join(out, `${width}-landing-end.png`) });

await click("nav-dashboard");
await page.waitForSelector(q("dash-balance"));
await shot("home");
await page.click(`${q("filters")} summary`);
await shot("filters");
await page.select(q("filter-category"), await page.$eval(`${q("filter-category")} option:nth-child(2)`, (o) => o.value));
await shot("filters-set");
await click("filter-clear");
await page.click(`${q("filters")} summary`);
await click("tab-monthly");
await shot("monthly");
await click("tab-summary");
await shot("summary");
await click("tab-daily");
for (let i = 0; i < 31 && !(await page.$(q("tx-row"))); i++) await click("month-prev");
await click("tx-row");
await shot("detail");
await click("nav-stats");
await page.waitForSelector(q("pie-slice"));
await shot("stats");
await click("nav-add");
await shot("add");
for (let i = 0; i < 3; i++) await click("add-item");
await page.type(q("item-name"), "Mleko 3,2% 1l");
await page.type(q("item-price"), "4,29");
await page.$eval(q("add-item"), (b) => b.scrollIntoView({ block: "end" }));
await shot("add-items");
await click("nav-settings");
await shot("settings");
await click("manage-categories");
await shot("categories");
await click("nav-settings");
await click("theme-light");
await click("nav-dashboard");
await shot("home-light");

await browser.close();
console.log(`wrote screenshots to ${out}`);
process.exit(0);
