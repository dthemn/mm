// Makes the screenshots on the welcome page from made-up data: a fresh account, a synthetic bank statement,
// then Home in the dark theme and Stats in the light one.
// node e2e/shots.mjs   (needs the test DB, web/dist and target/debug/mm-server, like run.mjs)
// Writes web/static/shot-dark.webp and web/static/shot-light.webp.

import { join } from "node:path";
import { demo, root, settle, q } from "./demo.mjs";

const { browser, page, click, rows } = await demo();
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
