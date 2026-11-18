// App shell: header (page title + stats, like the desktop's page header),
// the page, and a bottom tab bar (the desktop's sidebar, for thumbs).
import { render } from 'preact';
import { useCallback, useEffect, useState } from 'preact/hooks';

import { prefetchAudio } from './audio.js';
import { Review } from './review.jsx';
import { PairCard, Settings } from './settings.jsx';
import { loadSrs, replay } from './srs.js';
import { kv } from './store.js';
import { NotPaired, ensurePaired, localState, sync } from './sync.js';
import { Icon } from './ui.jsx';
import { Words } from './words.jsx';
import './style.css';

function dayStart(now, hour = 4) {
  const d = new Date(now - hour * 3_600_000);
  d.setHours(0, 0, 0, 0);
  return d.getTime() + hour * 3_600_000;
}

/** Cards with schedules (Rust FSRS via wasm), plus today's queue. */
function buildQueue(cards, byCard, settings, now) {
  const config = settings ? { desired_retention: settings.desired_retention, max_interval_days: 3650 } : undefined;
  const items = cards.map((card) => ({ card, schedule: replay(byCard.get(card.id) || [], config) }));
  const start = dayStart(now);
  const newToday = [...byCard.values()].filter((logs) => logs.length && Math.min(...logs.map((l) => l.reviewed_at)) >= start).length;
  const limit = Math.max(0, (settings?.new_per_day ?? 20) - newToday);
  const due = items.filter((i) => i.schedule.phase !== 'new' && i.schedule.due <= now).sort((a, b) => a.schedule.due - b.schedule.due);
  const fresh = items.filter((i) => i.schedule.phase === 'new').sort((a, b) => a.card.created_at - b.card.created_at).slice(0, limit);
  const reviewedToday = [...byCard.values()].flat().filter((l) => l.reviewed_at >= start).length;
  return { items, queue: [...due, ...fresh], config, reviewedToday, fourButtons: !!settings?.four_buttons, autoplay: settings?.audio_autoplay !== false };
}

function useData() {
  const [data, setData] = useState(null);
  const [syncState, setSyncState] = useState({ busy: false, error: null, last: null });
  const [paired, setPaired] = useState(true);

  const reload = useCallback(async () => {
    const { cards, byCard } = await localState();
    const settings = await kv.get('srs_settings');
    const built = buildQueue(cards, byCard, settings, Date.now());
    setData({ ...built, byCard });
    // Today's cards get their audio ahead of time, for offline review.
    prefetchAudio(built.queue.map((i) => i.card)).catch(() => {});
  }, []);

  const doSync = useCallback(async () => {
    setSyncState((s) => ({ ...s, busy: true }));
    try {
      await sync();
      setPaired(true);
      setSyncState({ busy: false, error: null, last: Date.now() });
    } catch (e) {
      if (e instanceof NotPaired) setPaired(false);
      setSyncState({ busy: false, error: navigator.onLine ? e.message : '离线中：复习记录会在联网后自动同步', last: (await kv.get('last_sync')) || null });
    }
    await reload();
  }, [reload]);

  useEffect(() => {
    (async () => {
      await loadSrs();
      try {
        await ensurePaired();
      } catch (_) {
        setPaired(false);
      }
      await reload();
      kv.get('last_sync').then((last) => last && setSyncState((s) => ({ ...s, last: s.last || last })));
      doSync();
    })();
    const onFocus = () => document.visibilityState === 'visible' && doSync();
    document.addEventListener('visibilitychange', onFocus);
    window.addEventListener('online', doSync);
    return () => {
      document.removeEventListener('visibilitychange', onFocus);
      window.removeEventListener('online', doSync);
    };
  }, [reload, doSync]);

  return { data, reload, doSync, syncState, paired };
}

const TABS = [
  ['review', '复习'],
  ['words', '词单'],
  ['settings', '设置'],
];

function App() {
  const { data, reload, doSync, syncState, paired } = useData();
  const [tab, setTab] = useState('review');
  const [query, setQuery] = useState('');

  if (!data) {
    return (
      <div class="app">
        <div class="loading">载入中…</div>
      </div>
    );
  }
  const due = data.queue.length;
  const offline = paired && syncState.error && !syncState.busy;

  return (
    <div class="app">
      <header class="topbar">
        <h1>{TABS.find(([k]) => k === tab)[1]}</h1>
        <div class="stats">
          {tab === 'review' && (
            <>
              <span>
                待复习 <b>{due}</b>
              </span>
              <span>
                今日 <b>{data.reviewedToday}</b>
              </span>
            </>
          )}
          {tab === 'words' && (
            <span>
              词单 <b>{data.items.length}</b>
            </span>
          )}
          <span class={`sync${syncState.busy ? ' spinning' : ''}`} title={offline ? syncState.error : '同步'}>
            <Icon name={offline ? 'offline' : syncState.busy ? 'sync' : 'cloud'} size={16} />
          </span>
        </div>
      </header>
      <main key={tab}>
        {!paired && tab === 'review' && <PairCard doSync={doSync} />}
        {tab === 'review' && (paired || data.items.length > 0) && <Review data={data} reload={reload} doSync={doSync} />}
        {tab === 'words' && <Words data={data} query={query} setQuery={setQuery} />}
        {tab === 'settings' && <Settings syncState={syncState} doSync={doSync} paired={paired} />}
      </main>
      <nav class="tabbar">
        {TABS.map(([k, label]) => (
          <button key={k} class={tab === k ? 'on' : ''} onClick={() => setTab(k)} aria-current={tab === k ? 'page' : undefined}>
            <span class="tab-icon">
              <Icon name={k} size={22} />
              {k === 'review' && due > 0 && <span class="tab-badge">{due > 99 ? '99+' : due}</span>}
            </span>
            <span>{label}</span>
          </button>
        ))}
      </nav>
    </div>
  );
}

render(<App />, document.getElementById('app'));

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/app/sw.js', { scope: '/app/' }).catch(() => {});
}
