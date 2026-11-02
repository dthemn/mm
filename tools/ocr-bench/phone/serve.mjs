// node phone/serve.mjs [port] [--once]: serves the speed test page, writes results to phone/results.jsonl
import { createServer } from "node:http";
import { createReadStream, existsSync, appendFileSync, statSync } from "node:fs";
import { dirname, join, normalize, extname } from "node:path";
import { fileURLToPath } from "node:url";
import { networkInterfaces } from "node:os";
const here = dirname(fileURLToPath(import.meta.url)), cache = join(here, "../.cache");
const models = join(cache, "venv/lib/python3.14/site-packages/rapidocr/models");
const asset = { "receipt.jpg": process.env.BENCH_IMG, "pol_fast.traineddata": join(cache, "models/pol_fast.traineddata"),
  "det.onnx": join(models, "ch_PP-OCRv5_det_mobile.onnx"), "rec.onnx": join(models, "latin_PP-OCRv5_rec_mobile.onnx") };
const types = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".wasm": "application/wasm" };
const port = +(process.argv[2] ?? 8099), once = process.argv.includes("--once");
createServer((q, s) => {
  s.setHeader("Cross-Origin-Opener-Policy", "same-origin");
  s.setHeader("Cross-Origin-Embedder-Policy", "require-corp");
  const u = decodeURIComponent(q.url.split("?")[0]);
  if (q.method === "POST" && u === "/result") {
    let b = ""; q.on("data", (d) => (b += d));
    q.on("end", () => { appendFileSync(join(here, "results.jsonl"), b + "\n"); console.log(b); s.end("ok"); if (once) process.exit(0); });
    return;
  }
  let f = u === "/" ? join(here, "index.html") : u.startsWith("/asset/") ? asset[u.slice(7)]
    : u.startsWith("/dep/") ? join(cache, "node_modules", normalize(u.slice(5)).replace(/^(\.\.[/\\])+/, "")) : null;
  if (!f || !existsSync(f) || !statSync(f).isFile()) { s.statusCode = 404; return s.end(); }
  s.setHeader("Content-Type", types[extname(f)] ?? "application/octet-stream");
  createReadStream(f).pipe(s);
}).listen(port, "0.0.0.0", () => {
  for (const a of Object.values(networkInterfaces()).flat()) if (a.family === "IPv4") console.log(`http://${a.address}:${port}`);
});
