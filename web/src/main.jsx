import { render } from 'preact';
import { useCallback, useEffect, useMemo, useState } from 'preact/hooks';

import { Glossary, Ruby } from './glossary.jsx';
import { play, prefetchAudio } from './audio.js';
import { loadSrs, preview, replay } from './srs.js';
import { addReview, kv, resetAll } from './store.js';
import { NotPaired, deviceId, ensurePaired, localState, mediaUrl, pairWith, standalone, sync } from './sync.js';
import './style.css';

const DAY = 86_400_000;

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
      setSyncState({ busy: false, error: navigator.onLine ? e.message : '离线：复习记录会在联网后自动同步', last: (await kv.get('last_sync')) || null });
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

function Picture({ id }) {
  const [src, setSrc] = useState(null);
  useEffect(() => {
    let alive = true;
    if (id) mediaUrl(id).then((u) => alive && setSrc(u));
    return () => (alive = false);
  }, [id]);
  return src ? <img class="pic" src={src} alt="" loading="lazy" /> : null;
}

function Explanation({ e }) {
  if (!e) return null;
  return (
    <section class="zh">
      {e.meaning_zh && <p>{e.meaning_zh}</p>}
      {e.gloss_zh && <p class="muted">义项：{e.gloss_zh}</p>}
      {e.sentence_zh && <p>译：{e.sentence_zh}</p>}
      {(e.examples || []).map((x, i) => (
        <div class="ex" key={i}>
          <p>{x.ja}</p>
          <p class="muted">{x.zh}</p>
        </div>
      ))}
    </section>
  );
}

const isKana = (c) => c >= '\u3041' && c <= '\u309f';

/** Character range of the card's word in its sentence, also when inflected (催す in 催していた). */
function highlightOf(card) {
  const { sentence, expression } = card;
  if (!sentence || !expression) return null;
  const toRange = (i, len) => {
    const s = [...sentence.slice(0, i)].length;
    return [s, s + len];
  };
  const i = sentence.indexOf(expression);
  if (i >= 0) return toRange(i, [...expression].length);
  const chars = [...expression];
  while (chars.length > 1 && isKana(chars[chars.length - 1])) chars.pop();
  const stem = chars.join('');
  if (stem === expression) return null;
  const j = sentence.indexOf(stem);
  if (j < 0) return null;
  let len = chars.length;
  const okurigana = [...expression].length - chars.length;
  for (const c of [...sentence.slice(j + stem.length)].slice(0, Math.max(1, okurigana))) {
    if (!isKana(c)) break;
    len++;
  }
  return toRange(j, len);
}

/** Speaker button; doesn't flip the card it sits on. */
function Say({ req, title }) {
  return (
    <button
      class="say"
      type="button"
      title={title}
      aria-label={title}
      onClick={(ev) => {
        ev.stopPropagation();
        play(req);
      }}
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5z" />
        <path d="M16 9a5 5 0 0 1 0 6" />
        <path d="M19.4 18.4a9 9 0 0 0 0-12.8" />
      </svg>
    </button>
  );
}

function Review({ data, reload, doSync }) {
  const [revealed, setRevealed] = useState(false);
  const [shownAt, setShownAt] = useState(Date.now());
  const item = data.queue[0];

  useEffect(() => {
    setRevealed(false);
    setShownAt(Date.now());
  }, [item?.card.id]);

  const sayWord = () => item && play({ e: item.card.expression, r: item.card.reading });
  const reveal = () => {
    if (revealed) return;
    setRevealed(true);
    if (data.autoplay) sayWord();
  };

  const labels = useMemo(() => (item && revealed ? preview(item.schedule, Date.now(), data.config).labels : null), [item, revealed, data.config]);

  const rate = async (rating) => {
    if (!item) return;
    const log = {
      id: crypto.randomUUID(),
      card_id: item.card.id,
      rating,
      reviewed_at: Date.now(),
      device_id: await deviceId(),
      duration_ms: Date.now() - shownAt,
    };
    await addReview(log);
    await reload();
    doSync();
  };

  useEffect(() => {
    const onKey = (e) => {
      if (e.key === ' ' || e.key === 'Enter') {
        e.preventDefault();
        // On the answer side, space means "remembered".
        if (revealed) rate(3);
        else reveal();
      } else if (revealed && (e.key === 'r' || e.key === 'R')) {
        sayWord();
      } else if (revealed && ['1', '2', '3', '4'].includes(e.key)) {
        const r = Number(e.key);
        rate(data.fourButtons || r === 1 ? r : 3);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  if (!item) {
    return (
      <div class="empty">
        <h2>今天的复习完成了</h2>
        <p class="muted">{data.items.length ? '新的复习会在到期后出现。' : '在电脑上查词并「加入词单」后，单词会同步到这里。'}</p>
      </div>
    );
  }
  const c = item.card;
  return (
    <div class="review">
      <article class="card" onClick={reveal}>
        <h1 class="word">{c.expression}</h1>
        {revealed && (
          <p class="reading">
            {c.reading}
            <Say title="发音（R）" req={{ e: c.expression, r: c.reading }} />
            {c.frequency && <span class="muted"> · 频率 {c.frequency}</span>}
          </p>
        )}
        {c.sentence &&
          (revealed ? (
            <div class="said">
              {c.sentence_furigana?.length ? <Ruby segments={c.sentence_furigana} highlight={highlightOf(c)} /> : <p class="sentence">{c.sentence}</p>}
              <Say title="朗读例句" req={{ s: c.sentence, e: c.expression, r: c.reading }} />
            </div>
          ) : (
            <p class="sentence">{c.sentence}</p>
          ))}
        <Picture id={c.media_id} />
        {revealed && (
          <>
            {(c.definitions || []).map((d, i) => (
              <section class="defs" key={i}>
                <p class="muted small">{d.dict}</p>
                <Glossary value={d.glossary} />
              </section>
            ))}
            <Explanation e={c.explanation} />
          </>
        )}
      </article>
      <div class={`answer${revealed && !data.fourButtons ? ' two' : ''}`}>
        {revealed && !data.fourButtons ? (
          [
            ['忘了', 1, 'again'],
            ['记得', 3, 'good'],
          ].map(([name, r, cls]) => (
            <button key={r} class={`rate ${cls}`} onClick={() => rate(r)}>
              <span>{name}</span>
              <small>{labels?.[r - 1]}后</small>
            </button>
          ))
        ) : revealed ? (
          [
            ['重来', 1, 'again'],
            ['困难', 2, 'hard'],
            ['良好', 3, 'good'],
            ['简单', 4, 'easy'],
          ].map(([name, r, cls]) => (
            <button key={r} class={`rate ${cls}`} onClick={() => rate(r)}>
              <span>{name}</span>
              <small>{labels?.[r - 1]}</small>
            </button>
          ))
        ) : (
          <button class="reveal" onClick={reveal}>
            显示答案
          </button>
        )}
      </div>
    </div>
  );
}

function Words({ data }) {
  const [q, setQ] = useState('');
  const [open, setOpen] = useState(null);
  const now = Date.now();
  const shown = data.items
    .filter(({ card: c }) => !q || c.expression.includes(q) || c.reading.includes(q) || (c.sentence || '').includes(q) || (c.explanation?.meaning_zh || '').includes(q))
    .sort((a, b) => b.card.created_at - a.card.created_at);
  const due = (s) => (s.phase === 'new' ? '新词' : s.due <= now ? '待复习' : `${Math.max(1, Math.round((s.due - now) / DAY))}天后`);
  return (
    <div class="words">
      <input class="search" type="search" placeholder="搜索单词、读音、中文意思" value={q} onInput={(e) => setQ(e.currentTarget.value)} />
      <p class="muted small">{shown.length} / {data.items.length} 词</p>
      <ul>
        {shown.map(({ card: c, schedule: s }) => (
          <li key={c.id}>
            <button class="row" onClick={() => setOpen(open === c.id ? null : c.id)}>
              <strong>{c.expression}</strong>
              <span class="muted">{c.reading}</span>
              <span class="meaning">{c.explanation?.meaning_zh || ''}</span>
              <span class="muted small">{due(s)}</span>
            </button>
            {open === c.id && (
              <div class="detail">
                <p class="reading">
                  {c.reading}
                  <Say title="发音" req={{ e: c.expression, r: c.reading }} />
                </p>
                {c.sentence_furigana?.length > 0 && (
                  <div class="said">
                    <Ruby segments={c.sentence_furigana} highlight={highlightOf(c)} />
                    <Say title="朗读例句" req={{ s: c.sentence, e: c.expression, r: c.reading }} />
                  </div>
                )}
                <Explanation e={c.explanation} />
                <p class="muted small">复习 {s.reps} 次 · 遗忘 {s.lapses} 次</p>
              </div>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Pairing for a phone that has no token yet (e.g. a new home-screen app). */
function PairCard({ doSync }) {
  const [text, setText] = useState('');
  const [error, setError] = useState(null);
  const pair = async (value) => {
    try {
      setError(null);
      await pairWith(value);
      await doSync();
    } catch (e) {
      setError(e.message);
    }
  };
  const paste = async () => {
    try {
      const t = await navigator.clipboard.readText();
      setText(t);
      await pair(t);
    } catch (_) {
      setError('读不到剪贴板：长按下面的输入框选「粘贴」，再点「配对」。');
    }
  };
  return (
    <section class="pair">
      <h3>和电脑配对</h3>
      <p class="muted small">
        {standalone()
          ? '主屏幕上的 App 和浏览器的数据是分开的，需要再配对一次：在电脑 pic_yomi →「设置 → 手机同步」点配对链接旁的「复制」，再回到这里粘贴。'
          : '在电脑 pic_yomi →「设置 → 手机同步」用相机扫二维码；或者点配对链接旁的「复制」，再回到这里粘贴。'}
      </p>
      <button class="primary" onClick={paste}>从剪贴板粘贴配对链接</button>
      <div class="row">
        <input placeholder="或者粘贴到这里" value={text} onInput={(e) => setText(e.currentTarget.value)} />
        <button onClick={() => pair(text)} disabled={!text.trim()}>配对</button>
      </div>
      {error && <p class="error small">{error}</p>}
    </section>
  );
}

function Settings({ syncState, doSync, paired }) {
  return (
    <div class="settings">
      <section>
        <h3>同步</h3>
        <p class="muted">{paired ? '已配对电脑。' : '还没有和电脑配对。'}</p>
        <p class="muted small">
          {syncState.last ? `上次同步：${new Date(syncState.last).toLocaleString()}` : '尚未同步'}
          {syncState.error ? ` · ${syncState.error}` : ''}
        </p>
        <button onClick={doSync} disabled={syncState.busy}>
          {syncState.busy ? '同步中…' : '立即同步'}
        </button>
      </section>
      {!paired && <PairCard doSync={doSync} />}
      <section>
        <h3>本机数据</h3>
        <p class="muted small">复习记录保存在手机上，联网后自动同步到电脑。清除后会从电脑重新下载。</p>
        <button
          class="danger"
          onClick={async () => {
            if (confirm('清除本机缓存的词单和复习记录？未同步的复习会丢失。')) {
              await resetAll();
              location.reload();
            }
          }}
        >
          清除本机数据
        </button>
      </section>
    </div>
  );
}

function App() {
  const { data, reload, doSync, syncState, paired } = useData();
  const [tab, setTab] = useState('review');
  if (!data) return <div class="empty">载入中…</div>;
  return (
    <div class="app">
      <header>
        <span class="brand">pic_yomi</span>
        <span class="muted small">
          待复习 {data.queue.length} · 今日 {data.reviewedToday}
          {syncState.busy ? ' · 同步中' : ''}
        </span>
      </header>
      <main>
        {!paired && tab === 'review' && <PairCard doSync={doSync} />}
        {tab === 'review' && (paired || data.items.length > 0) && <Review data={data} reload={reload} doSync={doSync} />}
        {tab === 'words' && <Words data={data} />}
        {tab === 'settings' && <Settings syncState={syncState} doSync={doSync} paired={paired} />}
      </main>
      <nav>
        {[
          ['review', '复习'],
          ['words', '词单'],
          ['settings', '设置'],
        ].map(([k, label]) => (
          <button key={k} class={tab === k ? 'on' : ''} onClick={() => setTab(k)}>
            {label}
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
