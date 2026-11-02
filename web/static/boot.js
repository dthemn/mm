import init from "/web.js";

await init({ module_or_path: fetch("/web_bg.wasm", { integrity: "__WASM_SRI__" }) });

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js");
}
