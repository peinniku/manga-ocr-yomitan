// Pronunciation playback through Web Audio.
// iOS only lets audio start inside a tap, so the context is unlocked on the first
// tap/key and stays usable afterwards (also for auto-play after a reveal).
// Clips come from /api/audio; the service worker keeps them for offline review.

import { kv } from './store.js';

let ctx = null;
let playing = null;
const decoded = new Map();

export function unlockAudio() {
  try {
    if (navigator.audioSession) navigator.audioSession.type = 'playback'; // iOS 17+: play even with the silent switch on
    ctx = ctx || new (window.AudioContext || window.webkitAudioContext)();
    if (ctx.state === 'suspended') ctx.resume();
  } catch (_) {}
}

for (const ev of ['pointerdown', 'keydown', 'touchend']) window.addEventListener(ev, unlockAudio, { capture: true, passive: true });

export async function audioUrl({ e = '', r = '', s = '' }) {
  const [token, settings] = await Promise.all([kv.get('token'), kv.get('srs_settings')]);
  // The voice is part of the URL so the offline cache follows a voice change.
  const q = new URLSearchParams({ e, r, s, v: settings?.voice || '', t: token || '' });
  return `/api/audio?${q}`;
}

async function clip(req) {
  const url = await audioUrl(req);
  const key = url.replace(/&t=[^&]*/, '');
  if (decoded.has(key)) return decoded.get(key);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`没有找到发音（${res.status}）`);
  const bytes = await res.arrayBuffer();
  unlockAudio();
  const buf = await new Promise((ok, fail) => ctx.decodeAudioData(bytes, ok, fail));
  decoded.set(key, buf);
  return buf;
}

/** Play a word (`{e, r}`) or a sentence (`{s, e, r}`); a newer request replaces the current one. */
let latest = 0;
export async function play(req) {
  const id = ++latest;
  unlockAudio();
  try {
    const buf = await clip(req);
    if (id !== latest) return;
    if (playing) try { playing.stop(); } catch (_) {}
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(ctx.destination);
    src.start();
    playing = src;
  } catch (err) {
    console.warn('audio', err);
  }
}

/** Fetch clips ahead of time (the service worker caches them for offline use). */
let prefetching = false;
export async function prefetchAudio(cards) {
  if (prefetching || !navigator.serviceWorker?.controller) return;
  prefetching = true;
  try {
    await prefetchAll(cards);
  } finally {
    prefetching = false;
  }
}

async function prefetchAll(cards) {
  const done = new Set((await kv.get('audio_cached')) || []);
  for (const c of cards) {
    for (const req of [{ e: c.expression, r: c.reading }, c.sentence ? { s: c.sentence, e: c.expression, r: c.reading } : null]) {
      if (!req) continue;
      const url = (await audioUrl(req)).replace(/&t=[^&]*/, '');
      if (done.has(url)) continue;
      try {
        const res = await fetch(await audioUrl(req));
        if (res.ok) {
          await res.arrayBuffer();
          done.add(url);
          await kv.set('audio_cached', [...done]);
        }
      } catch (_) {
        return; // offline: try again next sync
      }
    }
  }
  await kv.set('audio_cached', [...done]);
}
