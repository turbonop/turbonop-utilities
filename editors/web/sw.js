// Cross-origin isolation for a host that can't send headers (GitHub
// Pages): this service worker gives the site's own responses the
// Cross-Origin-Opener-Policy and Cross-Origin-Embedder-Policy headers that
// make the page cross-origin isolated, which SharedArrayBuffer (and so
// hexcavator's threads) needs. (app.js registers it, and reloads once it
// controls the page.)
"use strict";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

self.addEventListener("fetch", (e) => {
  const r = e.request;
  if (r.cache === "only-if-cached" && r.mode !== "same-origin") {
    return;
  }
  // (Only the site's own: anything else goes as it would.)
  if (new URL(r.url).origin !== self.location.origin) {
    return;
  }
  e.respondWith(
    fetch(r).then((res) => {
      if (res.status === 0) {
        return res;
      }
      const headers = new Headers(res.headers);
      headers.set("Cross-Origin-Opener-Policy", "same-origin");
      headers.set("Cross-Origin-Embedder-Policy", "require-corp");
      headers.set("Cross-Origin-Resource-Policy", "same-origin");
      return new Response(res.body, { status: res.status, statusText: res.statusText, headers });
    })
  );
});
