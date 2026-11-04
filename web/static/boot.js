import init from "/web.js?v=__JS_V__";

// The theme is applied before the app starts, so the page does not flash. Dark unless the user chose light.
document.documentElement.dataset.theme = localStorage.getItem("mmTheme") === "light" ? "light" : "dark";

await init({ module_or_path: fetch("/web_bg.wasm?v=__WASM_V__", { integrity: "__WASM_SRI__" }) });

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js");
}
