// pic_yomi extension background: talks to the desktop app's local API (127.0.0.1:7331).
// Content scripts never call the API directly; they message this worker, which holds the token.

const API = 'http://127.0.0.1:7331';
const api = typeof browser !== 'undefined' ? browser : chrome;

let token = null;

async function loadToken() {
  if (token) return token;
  const stored = await api.storage.local.get('token');
  if (stored.token) {
    token = stored.token;
    return token;
  }
  return pair();
}

/** Ask the desktop app for its pairing token (only answered for callers on this Mac). */
async function pair() {
  const r = await fetch(`${API}/api/pair-local`);
  if (!r.ok) throw new Error(`pairing failed (${r.status})`);
  token = (await r.json()).token;
  await api.storage.local.set({ token });
  return token;
}

async function call(path, { method = 'GET', body } = {}, retry = true) {
  const t = await loadToken();
  let r;
  try {
    r = await fetch(API + path, {
      method,
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${t}` },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    throw new Error('pic_yomi 桌面程序没有运行');
  }
  if (r.status === 401 && retry) {
    token = null;
    await api.storage.local.remove('token');
    await pair();
    return call(path, { method, body }, false);
  }
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
  return data;
}

/** X serves downscaled images by default; OCR reads the original much better. */
function preferOriginal(src) {
  try {
    const u = new URL(src);
    if (u.hostname === 'pbs.twimg.com' && u.pathname.startsWith('/media/')) {
      u.searchParams.set('name', 'orig');
      return u.href;
    }
  } catch (_) {}
  return src;
}

function toBase64(buf) {
  const bytes = new Uint8Array(buf);
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

async function fetchImage(src) {
  const preferred = preferOriginal(src);
  for (const url of preferred === src ? [src] : [preferred, src]) {
    try {
      const r = await fetch(url, { credentials: 'include' });
      if (r.ok) return { key: url, data: toBase64(await r.arrayBuffer()) };
    } catch (_) {}
  }
  throw new Error('无法下载图片');
}

const handlers = {
  async ocr({ src, dataUrl }) {
    if (dataUrl) return { key: src, page: await call('/api/ocr', { method: 'POST', body: { image: dataUrl, key: src } }) };
    const img = await fetchImage(src);
    return { key: img.key, page: await call('/api/ocr', { method: 'POST', body: { image: img.data, key: img.key } }) };
  },
  lookup: ({ text, offset }) => call('/api/lookup', { method: 'POST', body: { text, offset } }),
  explain: (req) => call('/api/explain', { method: 'POST', body: req }),
  addCard: (req) => call('/api/cards', { method: 'POST', body: req }),
  stats: () => call('/api/stats'),
  health: async () => (await fetch(`${API}/health`)).json(),
};

api.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  const h = handlers[msg && msg.type];
  if (!h) return false;
  Promise.resolve(h(msg.payload || {}))
    .then((data) => sendResponse({ ok: true, data }))
    .catch((e) => sendResponse({ ok: false, error: String(e.message || e) }));
  return true; // async response
});
