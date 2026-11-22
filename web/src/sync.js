// Pairing, API calls, and two-way sync with the desktop app.

import { allCards, allLogs, clearOutbox, kv, outbox, putCards, putLogs } from './store.js';

export class NotPaired extends Error {}

export async function deviceId() {
  let id = await kv.get('device_id');
  if (!id) {
    id = `phone-${crypto.randomUUID().slice(0, 8)}`;
    await kv.set('device_id', id);
  }
  return id;
}

/** Running as a home-screen app (not in a browser tab). */
export const standalone = () => matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;

/**
 * In a browser tab, keep the pairing in the address bar. "Add to Home Screen"
 * starts the new app from this address, and on iOS a home-screen app has its
 * own storage, so this is how it gets paired too. Home-screen apps drop it.
 */
function keepPairInUrl(token) {
  const want = location.pathname + (standalone() ? '' : `#pair=${token}`);
  if (location.pathname + location.hash !== want) history.replaceState(null, '', want);
}

/** The token in a pairing link (…#pair=XYZ), or a bare token. */
export function parseToken(text) {
  const m = (text || '').match(/pair=([\w-]+)/);
  if (m) return m[1];
  const t = (text || '').trim();
  return /^[\w-]{16,}$/.test(t) ? t : null;
}

/** Pair with a pasted link or token. */
export async function pairWith(text) {
  const token = parseToken(text);
  if (!token) throw new Error('这不是配对链接：在电脑 pic_yomi →「设置 → 手机同步」点配对链接旁的「复制」');
  await kv.set('token', token);
  keepPairInUrl(token);
  return token;
}

/** Take a token from `#pair=…` (QR code) or, on this Mac, from the desktop app directly. */
export async function ensurePaired() {
  const m = location.hash.match(/pair=([\w-]+)/);
  if (m) await kv.set('token', m[1]);
  let token = await kv.get('token');
  if (!token && ['127.0.0.1', 'localhost'].includes(location.hostname)) {
    const r = await fetch('/api/pair-local');
    if (r.ok) {
      token = (await r.json()).token;
      await kv.set('token', token);
    }
  }
  if (!token) throw new NotPaired('未配对');
  keepPairInUrl(token);
  return token;
}

export async function api(path, opts = {}) {
  const token = await ensurePaired();
  const r = await fetch(path, {
    ...opts,
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}`, ...(opts.headers || {}) },
  });
  if (r.status === 401) {
    await kv.del('token');
    throw new NotPaired('配对已失效');
  }
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
  return data;
}

export async function mediaUrl(id) {
  const token = await kv.get('token');
  return `/api/media/${id}?t=${encodeURIComponent(token || '')}`;
}

let running = null;

/** Push the outbox, pull everything newer than our cursor. Returns true if anything changed. */
export function sync() {
  if (running) return running;
  running = (async () => {
    try {
      const pending = await outbox();
      const since = (await kv.get('seq')) || 0;
      const res = await api('/api/sync', {
        method: 'POST',
        body: JSON.stringify({
          device_id: await deviceId(),
          since,
          cards: pending.filter((p) => p.kind === 'card').map((p) => p.data),
          logs: pending.filter((p) => p.kind === 'log').map((p) => p.data),
        }),
      });
      await putCards(res.cards);
      await putLogs(res.logs);
      await clearOutbox(pending.map((p) => p.id));
      await kv.set('seq', res.seq);
      await kv.set('last_sync', Date.now());
      try {
        await kv.set('srs_settings', await api('/api/settings/srs'));
      } catch (_) {}
      prefetchMedia().catch(() => {});
      return res.cards.length + res.logs.length > 0 || pending.length > 0;
    } finally {
      running = null;
    }
  })();
  return running;
}

/** Download card pictures while online so the service worker can serve them offline. */
async function prefetchMedia() {
  // Only the service worker stores fetched pictures; without it, try again next sync.
  if (!navigator.serviceWorker?.controller) return;
  const done = new Set((await kv.get('media_cached')) || []);
  const ids = (await allCards()).filter((c) => !c.deleted && c.media_id && !done.has(c.media_id)).map((c) => c.media_id);
  for (const id of ids) {
    const r = await fetch(await mediaUrl(id));
    if (r.ok) {
      await r.arrayBuffer();
      done.add(id);
    }
  }
  await kv.set('media_cached', [...done]);
}

/** Cards (not deleted) with their logs grouped. */
export async function localState() {
  const [cards, logs] = await Promise.all([allCards(), allLogs()]);
  const byCard = new Map();
  for (const l of logs) {
    if (!byCard.has(l.card_id)) byCard.set(l.card_id, []);
    byCard.get(l.card_id).push(l);
  }
  return { cards: cards.filter((c) => !c.deleted), byCard };
}
