// Runs the recognizer off the main thread. It receives only pixel tensors, never a photo file,
// and keeps nothing between requests except the loaded models.
import * as ort from "/ocr/ort.wasm.min.mjs";

ort.env.wasm.wasmPaths = "/ocr/";
ort.env.wasm.numThreads = self.crossOriginIsolated ? Math.min(4, navigator.hardwareConcurrency || 2) : 1;

const sessions = {};
// A failed load is forgotten, so a later request tries again instead of getting the same error.
const session = (name) => (sessions[name] ??= ort.InferenceSession.create(`/ocr/${name}.onnx`, { executionProviders: ["wasm"] })
  .catch((e) => { delete sessions[name]; throw e; }));

self.onmessage = async ({ data: { id, op, input, h, w } }) => {
  try {
    const s = await session(op);
    if (!input) {
      self.postMessage({ id, ready: true });
      return;
    }
    const out = await s.run({ [s.inputNames[0]]: new ort.Tensor("float32", input, [1, 3, h, w]) });
    const t = out[s.outputNames[0]];
    self.postMessage({ id, data: t.data, dims: t.dims }, [t.data.buffer]);
  } catch (e) {
    self.postMessage({ id, error: String(e) });
  }
};
