// Review: one card at a time, laid out like the desktop app. Both sides share
// one layout (the reading and furigana are laid out but hidden on the front),
// so turning the card only fades things in.
import { useEffect, useMemo, useState } from 'preact/hooks';

import { loadClip, play } from './audio.js';
import { Explanation, Picture, frontSegments, highlightOf } from './card.jsx';
import { Glossary, Ruby } from './glossary.jsx';
import { preview } from './srs.js';
import { addReview } from './store.js';
import { deviceId } from './sync.js';
import { Empty, Say } from './ui.jsx';

export function Review({ data, reload, doSync }) {
  const [revealed, setRevealed] = useState(false);
  const [shownAt, setShownAt] = useState(Date.now());
  const [busy, setBusy] = useState(false);
  const item = data.queue[0];

  useEffect(() => {
    setRevealed(false);
    setShownAt(Date.now());
    document.querySelector('main')?.scrollTo({ top: 0 });
    // In memory before the tap, so the tap can start it (iOS needs that).
    if (item) {
      const { expression: e, reading: r, sentence: s } = item.card;
      loadClip({ e, r }).catch(() => {});
      if (s) loadClip({ s, e, r }).catch(() => {});
    }
  }, [item?.card.id]);

  const sayWord = () => item && play({ e: item.card.expression, r: item.card.reading });
  const reveal = () => {
    if (revealed) return;
    setRevealed(true);
    if (data.autoplay) sayWord();
  };

  const labels = useMemo(() => (item && revealed ? preview(item.schedule, Date.now(), data.config).labels : null), [item, revealed, data.config]);

  const rate = async (rating) => {
    if (!item || busy) return;
    setBusy(true);
    try {
      await addReview({
        id: crypto.randomUUID(),
        card_id: item.card.id,
        rating,
        reviewed_at: Date.now(),
        device_id: await deviceId(),
        duration_ms: Date.now() - shownAt,
      });
      await reload();
      doSync();
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    const onKey = (e) => {
      if (e.target instanceof HTMLInputElement) return;
      if (e.key === ' ' || e.key === 'Enter') {
        e.preventDefault();
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
    return data.items.length ? (
      <Empty title="今天的复习完成了 🎉" hint="新的复习到期后会出现在这里。" />
    ) : (
      <Empty title="词单还是空的" hint="在电脑上查词并「加入词单」后，单词会同步到这里。" />
    );
  }

  const c = item.card;
  const hl = highlightOf(c);
  const buttons = data.fourButtons
    ? [
        ['重来', 1, 'danger'],
        ['困难', 2, 'outline'],
        ['良好', 3, 'primary'],
        ['简单', 4, 'outline'],
      ]
    : [
        ['忘了', 1, 'danger'],
        ['记得', 3, 'primary'],
      ];

  return (
    <div class="review">
      <article key={c.id} class={`card${revealed ? ' revealed' : ''}`} onClick={reveal}>
        <h1 class="word">{c.expression}</h1>
        <p class="reading">
          <span>{c.reading}</span>
          <Say title="发音（R）" req={{ e: c.expression, r: c.reading }} />
          {c.frequency && <span class="freq">频率 {c.frequency}</span>}
        </p>
        {c.sentence && (
          <div class={`said${[...c.sentence].length > 40 ? ' long' : ''}`}>
            {c.sentence_furigana?.length ? (
              <Ruby segments={revealed ? c.sentence_furigana : frontSegments(c, hl)} highlight={hl} />
            ) : (
              <p class="sentence">{c.sentence}</p>
            )}
            <Say title="朗读例句" req={{ s: c.sentence, e: c.expression, r: c.reading }} />
          </div>
        )}
        <Picture id={c.media_id} />
        {revealed && (
          <div class="answer-body">
            {(c.definitions || []).map((d, i) => (
              <section class="defs" key={i}>
                <p class="label">{d.dict}</p>
                <Glossary value={d.glossary} />
              </section>
            ))}
            <Explanation e={c.explanation} />
          </div>
        )}
      </article>

      <div class={`answer-bar${revealed ? ' revealed' : ''}`}>
        {revealed ? (
          <div class={`rates n${buttons.length}`}>
            {buttons.map(([name, r, kind]) => (
              <div class="rate" key={r}>
                <span class="when">{labels?.[r - 1]}后</span>
                <button class={`btn ${kind}`} disabled={busy} onClick={() => rate(r)}>
                  {name}
                </button>
              </div>
            ))}
          </div>
        ) : (
          <button class="btn primary wide" onClick={reveal}>
            显示答案
          </button>
        )}
      </div>
    </div>
  );
}
