// Pronunciation playback: clips are decoded with Web Audio and played into a
// MediaStream that one <audio> element plays.
// Why the detour (WebKit, iOS):
//  - an <audio> element is "media playback", so it is heard with the silent
//    switch on (plain Web Audio is "ambient" and silenced);
//  - an <audio> element with a normal source longer than 0.95 s makes the page
//    a Now Playing app (Dynamic Island, lock screen), and after that every clip
//    shows up there; an element playing a MediaStream never does.
// iOS only lets audio start inside a tap: play() wakes the element and context
// synchronously in the tap that asked for it.
// Clips come from /api/audio; the service worker keeps them for offline review.

import { kv } from './store.js';

let ctx = null;
let out = null; // where clips are connected: the stream, or the speakers as a fallback
let el = null;
let playing = null;
let idleTimer = 0;
const loaded = new Map(); // request key -> AudioBuffer

function context() {
  if (!ctx) {
    ctx = new (window.AudioContext || window.webkitAudioContext)();
    out = ctx.destination;
    try {
      const dest = ctx.createMediaStreamDestination();
      el = new Audio();
      el.playsInline = true;
      el.srcObject = dest.stream;
      out = dest;
    } catch (_) {
      el = null;
    }
  }
  return ctx;
}

/** Start the context and the stream element; call inside a tap/key. */
export function unlockAudio() {
  try {
    clearTimeout(idleTimer);
    context();
    if (ctx.state !== 'running') ctx.resume().catch(() => {});
    if (el?.paused) el.play().catch(() => {});
  } catch (_) {}
}

/** Pause the stream element a little after the last clip, so the phone's own
 *  music is only interrupted while we speak. */
function idleSoon() {
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    if (!playing && el && !el.paused) el.pause();
  }, 2000);
}

// The first tap unlocks audio (iOS), so later auto-play works.
let gestured = false;
for (const ev of ['touchend', 'click', 'keydown']) {
  window.addEventListener(
    ev,
    () => {
      if (gestured) return;
      gestured = true;
      unlockAudio();
      idleSoon();
    },
    { capture: true, passive: true },
  );
}

export async function audioUrl({ e = '', r = '', s = '' }) {
  const [token, settings] = await Promise.all([kv.get('token'), kv.get('srs_settings')]);
  // The voice is part of the URL so the offline cache follows a voice change.
  // `a` changes when the Mac makes clips differently (2: the card's reading is always spoken).
  const q = new URLSearchParams({ e, r, s, v: settings?.voice || '', a: '2', t: token || '' });
  return `/api/audio?${q}`;
}

const keyOf = ({ e = '', r = '', s = '' }) => `${e}|${r}|${s}`;

/** Fetch and decode a clip into memory, so a later tap can start it at once. */
export async function loadClip(req) {
  const k = keyOf(req);
  if (loaded.has(k)) return loaded.get(k);
  const res = await fetch(await audioUrl(req));
  if (!res.ok) throw new Error(`没有找到发音（${res.status}）`);
  const bytes = await res.arrayBuffer();
  const buf = await new Promise((ok, fail) => context().decodeAudioData(bytes, ok, fail));
  loaded.set(k, buf);
  return buf;
}

function start(buf) {
  if (playing) try { playing.stop(); } catch (_) {}
  if (ctx.state !== 'running') ctx.resume().catch(() => {});
  if (el?.paused) el.play().catch(() => {});
  const src = ctx.createBufferSource();
  src.buffer = buf;
  src.connect(out);
  src.onended = () => {
    if (playing === src) {
      playing = null;
      idleSoon();
    }
  };
  src.start();
  playing = src;
}

/** Play a word (`{e, r}`) or a sentence (`{s, e, r}`); a newer request replaces the current one. */
let latest = 0;
export function play(req) {
  const id = ++latest;
  unlockAudio(); // synchronous: still inside the tap
  const hit = loaded.get(keyOf(req));
  if (hit) return start(hit);
  loadClip(req)
    .then((buf) => id === latest && start(buf))
    .catch((err) => {
      console.warn('audio', err);
      if (!playing) idleSoon();
    });
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
