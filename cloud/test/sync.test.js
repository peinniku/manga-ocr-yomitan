// End-to-end checks against a running `wrangler dev` (local D1/R2).
// Run: npx wrangler dev --local (with TOKEN=local-test-token in .dev.vars), then npm test.
import { test } from 'node:test';
import assert from 'node:assert/strict';

const BASE = process.env.PICYOMI_CLOUD || 'http://127.0.0.1:8787';
const TOKEN = process.env.PICYOMI_TOKEN || 'local-test-token';
const auth = { authorization: `Bearer ${TOKEN}`, 'content-type': 'application/json' };
const sync = (body) => fetch(`${BASE}/api/sync`, { method: 'POST', headers: auth, body: JSON.stringify(body) }).then((r) => r.json());
const uid = () => crypto.randomUUID();

test('rejects requests without the token', async () => {
  assert.equal((await fetch(`${BASE}/api/sync`, { method: 'POST', body: '{}' })).status, 401);
  assert.equal((await fetch(`${BASE}/api/sync`, { method: 'POST', headers: { authorization: 'Bearer nope' }, body: '{}' })).status, 401);
  assert.equal((await fetch(`${BASE}/health`)).status, 200);
});

test('cards: last write wins; logs: deduplicated; cursor only moves on real changes', async () => {
  const start = (await sync({ since: 0 })).seq;
  const id = uid();
  const card = { id, expression: '綻ぶ', reading: 'ほころぶ', updated_at: 1000, created_at: 1000 };
  const log = { id: uid(), card_id: id, rating: 3, reviewed_at: 2000, device_id: 'phone' };

  let r = await sync({ since: start, cards: [card], logs: [log] });
  assert.ok(r.seq > start);
  assert.deepEqual(r.cards.map((c) => c.id), [id]);
  assert.equal(r.logs.length, 1);
  const after = r.seq;

  // The same card and log again (an echo from another device): nothing changes.
  r = await sync({ since: after, cards: [card], logs: [log] });
  assert.equal(r.seq, after);
  assert.equal(r.cards.length + r.logs.length, 0);

  // An older edit loses, a newer one wins.
  r = await sync({ since: after, cards: [{ ...card, reading: 'たんぶ', updated_at: 999 }] });
  assert.equal(r.cards.length, 0);
  r = await sync({ since: after, cards: [{ ...card, reading: 'ほころぶ!', updated_at: 1001 }] });
  assert.equal(r.cards[0].reading, 'ほころぶ!');
});

test('media, audio and settings round-trip', async () => {
  const bytes = new Uint8Array([1, 2, 3, 4]);
  const id = uid().replace(/-/g, '');
  assert.equal((await fetch(`${BASE}/api/media/${id}`, { method: 'PUT', headers: { authorization: auth.authorization, 'content-type': 'image/jpeg' }, body: bytes })).status, 200);
  const m = await fetch(`${BASE}/api/media/${id}?t=${TOKEN}`);
  assert.equal(m.headers.get('content-type'), 'image/jpeg');
  assert.deepEqual(new Uint8Array(await m.arrayBuffer()), bytes);

  const q = new URLSearchParams({ e: `綻ぶ${id}`, r: 'ほころぶ', s: '', v: 'ja-JP-NanamiNeural', a: '2' });
  assert.equal((await fetch(`${BASE}/api/audio?${q}`, { headers: auth })).status, 404);
  await fetch(`${BASE}/api/audio?${q}`, { method: 'PUT', headers: { authorization: auth.authorization, 'content-type': 'audio/mpeg' }, body: bytes });
  q.set('t', TOKEN);
  const a = await fetch(`${BASE}/api/audio?${q}`);
  assert.equal(a.status, 200);
  assert.equal(a.headers.get('content-type'), 'audio/mpeg');

  await fetch(`${BASE}/api/settings/srs`, { method: 'PUT', headers: auth, body: JSON.stringify({ desired_retention: 0.9, voice: 'x' }) });
  assert.equal((await (await fetch(`${BASE}/api/settings/srs`, { headers: auth })).json()).voice, 'x');
});

test('serves the web app', async () => {
  const r = await fetch(`${BASE}/`, { redirect: 'manual' });
  assert.equal(r.status, 302);
  const app = await fetch(`${BASE}/app/`);
  assert.equal(app.status, 200);
  assert.match(await app.text(), /<div id="app"|<script/);
});
