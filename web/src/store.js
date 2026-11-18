// Offline store (IndexedDB): cards, review logs, pending outbox, and small key/values.

const DB_NAME = 'picyomi';
const VERSION = 1;
let dbp = null;

function open() {
  if (dbp) return dbp;
  dbp = new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      db.createObjectStore('cards', { keyPath: 'id' });
      const logs = db.createObjectStore('logs', { keyPath: 'id' });
      logs.createIndex('card_id', 'card_id');
      db.createObjectStore('outbox', { keyPath: 'id' }); // {id, kind: 'log'|'card', data}
      db.createObjectStore('kv');
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  return dbp;
}

function wrap(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function tx(stores, mode, fn) {
  const db = await open();
  return new Promise((resolve, reject) => {
    const t = db.transaction(stores, mode);
    let result;
    Promise.resolve(fn(t)).then((r) => (result = r), reject);
    t.oncomplete = () => resolve(result);
    t.onerror = () => reject(t.error);
    t.onabort = () => reject(t.error);
  });
}

export const kv = {
  get: (key) => tx(['kv'], 'readonly', (t) => wrap(t.objectStore('kv').get(key))),
  set: (key, value) => tx(['kv'], 'readwrite', (t) => t.objectStore('kv').put(value, key)),
  del: (key) => tx(['kv'], 'readwrite', (t) => t.objectStore('kv').delete(key)),
};

export const allCards = () => tx(['cards'], 'readonly', (t) => wrap(t.objectStore('cards').getAll()));
export const allLogs = () => tx(['logs'], 'readonly', (t) => wrap(t.objectStore('logs').getAll()));
export const logsFor = (cardId) => tx(['logs'], 'readonly', (t) => wrap(t.objectStore('logs').index('card_id').getAll(cardId)));
export const outbox = () => tx(['outbox'], 'readonly', (t) => wrap(t.objectStore('outbox').getAll()));

export function putCards(cards) {
  return tx(['cards'], 'readwrite', (t) => {
    const s = t.objectStore('cards');
    for (const c of cards) s.put(c);
  });
}

export function putLogs(logs) {
  return tx(['logs'], 'readwrite', (t) => {
    const s = t.objectStore('logs');
    for (const l of logs) s.put(l);
  });
}

/** Record a review locally and queue it for upload, atomically. */
export function addReview(log) {
  return tx(['logs', 'outbox'], 'readwrite', (t) => {
    t.objectStore('logs').put(log);
    t.objectStore('outbox').put({ id: `log:${log.id}`, kind: 'log', data: log });
  });
}

export function queueCard(card) {
  return tx(['cards', 'outbox'], 'readwrite', (t) => {
    t.objectStore('cards').put(card);
    t.objectStore('outbox').put({ id: `card:${card.id}`, kind: 'card', data: card });
  });
}

export function clearOutbox(ids) {
  return tx(['outbox'], 'readwrite', (t) => {
    const s = t.objectStore('outbox');
    for (const id of ids) s.delete(id);
  });
}

export async function resetAll() {
  const db = await open();
  db.close();
  dbp = null;
  await wrap(indexedDB.deleteDatabase(DB_NAME));
}
