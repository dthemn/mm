// Runs tesseract-wasm under node: node tess_wasm.mjs jobs.json model.traineddata outdir
import { createRequire } from "node:module";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(here, ".cache/package.json"));
const { createOCREngine } = require("tesseract-wasm");
const wasm = readFileSync(join(here, ".cache/node_modules/tesseract-wasm/dist/tesseract-core.wasm"));

const [jobsPath, modelPath, outdir] = process.argv.slice(2);
const engine = await createOCREngine({ wasmBinary: wasm });
engine.loadModel(new Uint8Array(readFileSync(modelPath)));

const times = {};
for (const j of JSON.parse(readFileSync(jobsPath, "utf8"))) {
  const data = new Uint8ClampedArray(readFileSync(j.path));
  const t = performance.now();
  engine.loadImage({ data, width: j.w, height: j.h });
  // Words with coordinates: bench.py assembles lines the same way for all engines
  const words = engine.getTextBoxes("word");
  times[j.name] = (performance.now() - t) / 1000;
  writeFileSync(
    join(outdir, `${j.name}.boxes.json`),
    JSON.stringify(words.map((w) => [w.rect.left, w.rect.top, w.rect.bottom, w.text])),
  );
}
writeFileSync(join(outdir, "_times.json"), JSON.stringify(times));
