// App shell only. API responses are never cached: data lives in IndexedDB.
const CACHE = "mm-__VERSION__";
const FILES = ["/", "/index.html", "/app.css", "/boot.js", "/web.js", "/web_bg.wasm", "/manifest.webmanifest", "/icon.svg"];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(FILES)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin || url.pathname.startsWith("/api/")) return;
  const key = e.request.mode === "navigate" ? "/index.html" : url.pathname;
  e.respondWith(caches.match(key).then((r) => r || fetch(e.request)));
});
