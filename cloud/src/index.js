// pic_yomi cloud: the sync hub between the Mac and the phone.
//
// - /app/*            the phone web app (static assets)
// - POST /api/sync    cards (last write wins by updated_at) and review logs
//                     (append-only, deduplicated by id); same protocol the Mac
//                     app served before, so the web app needs no changes
// - /api/media/:id    card pictures; the Mac uploads them
// - /api/audio?…      pronunciation clips; the Mac uploads them
//   (both stored in D1: small files, and no R2 means no payment method needed)
// - /api/settings/srs review settings, pushed by the Mac
//
// Every /api call needs the pairing token (Bearer header, or ?t= for
// <img>/<audio> URLs). There is a single user: whoever holds the token.

const JSON_HEADERS = { 'content-type': 'application/json; charset=utf-8' };

const json = (v, status = 200) => new Response(JSON.stringify(v), { status, headers: JSON_HEADERS });

/** Constant-time comparison of two strings. */
function same(a, b) {
  if (typeof a !== 'string' || typeof b !== 'string' || a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

function authorized(request, url, env) {
  if (!env.TOKEN) return false;
  const header = request.headers.get('authorization') || '';
  const token = header.startsWith('Bearer ') ? header.slice(7) : url.searchParams.get('t');
  return same(token || '', env.TOKEN);
}

async function sha256(text) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

/** R2 key of a pronunciation clip: everything that changes the sound. */
async function audioKey(url) {
  const p = (k) => url.searchParams.get(k) || '';
  return `audio/${await sha256([p('v'), p('a'), p('s'), p('e'), p('r')].join('\u0000'))}`;
}

// The next change number, computed inside each statement so a batch stays ordered.
const NEXT_SEQ = '(SELECT COALESCE(MAX(s), 0) + 1 FROM (SELECT MAX(seq) AS s FROM cards UNION ALL SELECT MAX(seq) FROM review_logs))';

/** At most this many cards + logs per request (D1 limits queries per call). */
const MAX_ITEMS = 40;

async function sync(request, env) {
  const body = await request.json().catch(() => null);
  if (!body || typeof body !== 'object') return json({ error: 'bad request' }, 400);
  const cards = Array.isArray(body.cards) ? body.cards : [];
  const logs = Array.isArray(body.logs) ? body.logs : [];
  if (cards.length + logs.length > MAX_ITEMS) return json({ error: `at most ${MAX_ITEMS} items per request` }, 413);
  const since = Number(body.since) || 0;

  const writes = [];
  for (const c of cards) {
    if (!c || typeof c.id !== 'string' || typeof c.updated_at !== 'number') continue;
    writes.push(
      env.DB.prepare(
        `INSERT INTO cards(id, data, updated_at, deleted, seq) VALUES (?1, ?2, ?3, ?4, ${NEXT_SEQ})
         ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at,
           deleted = excluded.deleted, seq = excluded.seq
         WHERE excluded.updated_at > cards.updated_at`,
      ).bind(c.id, JSON.stringify(c), c.updated_at, c.deleted ? 1 : 0),
    );
  }
  for (const l of logs) {
    if (!l || typeof l.id !== 'string' || typeof l.card_id !== 'string') continue;
    writes.push(
      env.DB.prepare(
        `INSERT OR IGNORE INTO review_logs(id, card_id, data, reviewed_at, seq) VALUES (?1, ?2, ?3, ?4, ${NEXT_SEQ})`,
      ).bind(l.id, l.card_id, JSON.stringify(l), Number(l.reviewed_at) || 0),
    );
  }
  const reads = [
    env.DB.prepare('SELECT data FROM cards WHERE seq > ?1 ORDER BY seq').bind(since),
    env.DB.prepare('SELECT data FROM review_logs WHERE seq > ?1 ORDER BY seq').bind(since),
    env.DB.prepare('SELECT COALESCE(MAX(s), 0) AS seq FROM (SELECT MAX(seq) AS s FROM cards UNION ALL SELECT MAX(seq) FROM review_logs)'),
  ];
  // One batch = one transaction: writes, then a consistent read.
  const results = await env.DB.batch([...writes, ...reads]);
  const [cardRows, logRows, seqRow] = results.slice(writes.length);
  return json({
    seq: seqRow.results[0]?.seq ?? 0,
    cards: cardRows.results.map((r) => JSON.parse(r.data)),
    logs: logRows.results.map((r) => JSON.parse(r.data)),
  });
}

async function getObject(env, key) {
  const row = await env.DB.prepare('SELECT type, data FROM files WHERE key = ?1').bind(key).first();
  if (!row) return new Response('not found', { status: 404 });
  return new Response(new Uint8Array(row.data), {
    headers: { 'content-type': row.type || 'application/octet-stream', 'cache-control': 'private, max-age=604800' },
  });
}

async function putObject(request, env, key) {
  const type = request.headers.get('content-type') || 'application/octet-stream';
  const body = await request.arrayBuffer();
  // D1 rows are limited to 2 MB.
  if (body.byteLength === 0 || body.byteLength > 1900 * 1024) return json({ error: 'bad size' }, 400);
  await env.DB.prepare('INSERT OR REPLACE INTO files(key, type, data) VALUES (?1, ?2, ?3)').bind(key, type, body).run();
  return json({ ok: true });
}

async function api(request, env, url) {
  const { pathname } = url;
  const method = request.method;
  if (pathname === '/api/sync' && method === 'POST') return sync(request, env);

  if (pathname === '/api/settings/srs') {
    if (method === 'GET') {
      const row = await env.DB.prepare("SELECT value FROM meta WHERE key = 'srs_settings'").first();
      return row ? new Response(row.value, { headers: JSON_HEADERS }) : json({});
    }
    if (method === 'PUT') {
      const v = await request.json().catch(() => null);
      if (!v || typeof v !== 'object') return json({ error: 'bad request' }, 400);
      await env.DB.prepare("INSERT OR REPLACE INTO meta(key, value) VALUES ('srs_settings', ?1)").bind(JSON.stringify(v)).run();
      return json({ ok: true });
    }
  }

  const media = pathname.match(/^\/api\/media\/([\w-]{1,64})$/);
  if (media) {
    if (method === 'GET') return getObject(env, `media/${media[1]}`);
    if (method === 'PUT') return putObject(request, env, `media/${media[1]}`);
  }

  if (pathname === '/api/audio') {
    const key = await audioKey(url);
    if (method === 'GET') return getObject(env, key);
    if (method === 'PUT') return putObject(request, env, key);
  }

  return json({ error: 'not found' }, 404);
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === '/health') return json({ ok: true, app: 'pic_yomi-cloud' });
    if (url.pathname === '/' || url.pathname === '/app') return Response.redirect(`${url.origin}/app/`, 302);
    if (url.pathname.startsWith('/api/')) {
      if (!authorized(request, url, env)) return json({ error: 'unauthorized' }, 401);
      try {
        return await api(request, env, url);
      } catch (e) {
        return json({ error: String(e?.message || e) }, 500);
      }
    }
    // The web app; unknown paths under /app fall back to its index.
    return env.ASSETS.fetch(request);
  },
};
