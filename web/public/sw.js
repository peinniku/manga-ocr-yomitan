// Offline support: app shell is network-first (falls back to cache), card pictures are cache-first.
// Pronunciation clips are cache-first too. API calls other than media and audio always go to the network (the app keeps its own offline queue).
const SHELL = 'picyomi-shell-v1';
const MEDIA = 'picyomi-media-v1';
const AUDIO = 'picyomi-audio-v1';

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(SHELL).then((c) => c.addAll(['/app/', '/app/srs.wasm', '/app/manifest.webmanifest', '/app/icon-192.png'])));
  self.skipWaiting();
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys().then((keys) => Promise.all(keys.filter((k) => ![SHELL, MEDIA, AUDIO].includes(k)).map((k) => caches.delete(k)))).then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== 'GET' || url.origin !== location.origin) return;

  if (url.pathname.startsWith('/api/media/')) {
    // Media ids are immutable: cache by path, ignoring the token query.
    const key = new Request(url.origin + url.pathname);
    e.respondWith(
      caches.open(MEDIA).then(async (c) => {
        const hit = await c.match(key);
        if (hit) return hit;
        const res = await fetch(e.request);
        if (res.ok) c.put(key, res.clone());
        return res;
      }),
    );
    return;
  }
  if (url.pathname === '/api/audio') {
    // Clips depend only on word, sentence and voice: cache without the token.
    url.searchParams.delete('t');
    const key = new Request(url.href);
    e.respondWith(
      caches.open(AUDIO).then(async (c) => {
        const hit = await c.match(key);
        if (hit) return hit;
        const res = await fetch(e.request);
        if (res.ok) c.put(key, res.clone());
        return res;
      }),
    );
    return;
  }
  if (url.pathname.startsWith('/api/') || url.pathname === '/health') return;

  if (url.pathname.startsWith('/app')) {
    e.respondWith(
      fetch(e.request)
        .then((res) => {
          if (res.ok) caches.open(SHELL).then((c) => c.put(e.request, res.clone()));
          return res;
        })
        .catch(async () => (await caches.match(e.request)) || (await caches.match('/app/'))),
    );
  }
});
