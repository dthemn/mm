// App shell only. API responses are never cached: data lives in IndexedDB.
const CACHE = "mm-__VERSION__";
// The recognizer is large and changes rarely, so it lives in its own cache that survives app updates.
// Its files are fetched once, on the first scan. The name follows their hash, so a changed file replaces the old one.
const OCR = "mm-ocr-__OCR_VERSION__";
const FILES = ["/", "/index.html", "/app.css", "/boot.js", "/web.js", "/web_bg.wasm", "/manifest.webmanifest", "/icon.svg", "/ocr-worker.js"];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(FILES)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE && k !== OCR).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin || url.pathname.startsWith("/api/")) return;
  if (url.pathname.startsWith("/ocr/")) {
    e.respondWith(
      caches.open(OCR).then((c) =>
        c.match(url.pathname).then((hit) => hit || fetch(e.request).then((r) => {
          if (r.ok) c.put(url.pathname, r.clone());
          return r;
        })),
      ),
    );
    return;
  }
  const key = e.request.mode === "navigate" ? "/index.html" : url.pathname;
  e.respondWith(caches.match(key).then((r) => r || fetch(e.request)));
});
