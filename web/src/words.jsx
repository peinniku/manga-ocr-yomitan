// Word list, like the desktop: newest first, two-line rows with a due badge,
// tap a row to see its sentence, definitions and explanation.
import { useState } from 'preact/hooks';

import { Explanation, highlightOf } from './card.jsx';
import { Glossary, Ruby } from './glossary.jsx';
import { Empty, Icon, Say } from './ui.jsx';

const DAY = 86_400_000;

function dueBadge(s, now) {
  if (s.phase === 'new') return ['新词', 'new'];
  if (s.due <= now) return ['待复习', 'due'];
  const days = Math.max(1, Math.round((s.due - now) / DAY));
  return [days >= 30 ? `${Math.round(days / 30)}个月后` : `${days}天后`, ''];
}

export function Words({ data, query, setQuery }) {
  const [open, setOpen] = useState(null);
  const now = Date.now();
  const q = query.trim();
  const shown = data.items
    .filter(({ card: c }) => !q || c.expression.includes(q) || c.reading.includes(q) || (c.sentence || '').includes(q) || (c.explanation?.meaning_zh || '').includes(q))
    .sort((a, b) => b.card.created_at - a.card.created_at);

  return (
    <div class="words">
      <label class="search">
        <Icon name="search" size={16} />
        <input
          type="search"
          placeholder="搜索单词、读音或中文意思"
          value={query}
          enterkeyhint="search"
          autocapitalize="none"
          autocorrect="off"
          onInput={(e) => setQuery(e.currentTarget.value)}
        />
      </label>
      {data.items.length === 0 ? (
        <Empty title="词单还是空的" hint="在电脑上查词并「加入词单」后，单词会同步到这里。" />
      ) : shown.length === 0 ? (
        <Empty title="没有匹配的词" hint="单词、读音、例句或中文意思都能搜。" />
      ) : (
        <>
          <p class="count">{q ? `${shown.length} / ${data.items.length}` : data.items.length} 个词</p>
          <ul class="list">
            {shown.map(({ card: c, schedule: s }) => {
              const [badge, tone] = dueBadge(s, now);
              const isOpen = open === c.id;
              return (
                <li key={c.id} class={isOpen ? 'open' : ''}>
                  <button class="row" onClick={() => setOpen(isOpen ? null : c.id)} aria-expanded={isOpen}>
                    <span class="row-main">
                      <span class="row-top">
                        <strong>{c.expression}</strong>
                        <span class="muted">{c.reading}</span>
                      </span>
                      <span class="row-sub">{c.explanation?.meaning_zh || ''}</span>
                    </span>
                    <span class={`badge ${tone}`}>{badge}</span>
                  </button>
                  {isOpen && (
                    <div class="detail">
                      <div class="detail-head">
                        <strong>{c.expression}</strong>
                        <span class="muted">{c.reading}</span>
                        <Say title="发音" req={{ e: c.expression, r: c.reading }} />
                      </div>
                      {c.sentence_furigana?.length > 0 && (
                        <div class="said">
                          <Ruby segments={c.sentence_furigana} highlight={highlightOf(c)} />
                          <Say title="朗读例句" req={{ s: c.sentence, e: c.expression, r: c.reading }} />
                        </div>
                      )}
                      {(c.definitions || []).map((d, i) => (
                        <section class="defs" key={i}>
                          <p class="label">{d.dict}</p>
                          <Glossary value={d.glossary} limit={14} />
                        </section>
                      ))}
                      <Explanation e={c.explanation} />
                      <p class="foot">
                        复习 {s.reps} 次 · 遗忘 {s.lapses} 次
                      </p>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </>
      )}
    </div>
  );
}
