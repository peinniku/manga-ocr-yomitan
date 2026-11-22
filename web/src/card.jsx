// Pieces of a card shared by the review and word-list pages.
import { useEffect, useState } from 'preact/hooks';

import { mediaUrl } from './sync.js';

export function Picture({ id }) {
  const [src, setSrc] = useState(null);
  useEffect(() => {
    let alive = true;
    if (id) mediaUrl(id).then((u) => alive && setSrc(u));
    return () => (alive = false);
  }, [id]);
  return src ? <img class="pic" src={src} alt="" loading="lazy" /> : null;
}

/** The Chinese explanation box (secondary background, like the desktop). */
export function Explanation({ e }) {
  if (!e) return null;
  return (
    <section class="zh">
      <p class="label">中文解释</p>
      {e.meaning_zh && <p class="zh-main">{e.meaning_zh}</p>}
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

const isKana = (c) => c >= 'ぁ' && c <= 'ゟ';

/** Character range of the card's word in its furigana text. Computed on the
 *  Mac (inflections included); the fallback covers cards from before that. */
export function highlightOf(card) {
  if (card.highlight) return card.highlight;
  const { expression } = card;
  const sentence = card.sentence_furigana?.length ? card.sentence_furigana.map((s) => s.t).join('') : card.sentence;
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

const KANJI = /[㐀-䶿一-鿿々]/;

/** Furigana for the front: the word blanked out when the sentence spells it
 *  in kana (たるんでる for 弛む), which would give the reading away. */
export function frontSegments(card, hl) {
  const segs = card.sentence_furigana || [];
  if (!hl || !KANJI.test(card.expression)) return segs;
  const text = segs.map((s) => s.t).join('');
  if (KANJI.test([...text].slice(hl[0], hl[1]).join(''))) return segs;
  let pos = 0;
  return segs.map((s) => {
    const t = [...s.t].map((ch, i) => (pos + i >= hl[0] && pos + i < hl[1] ? '＿' : ch)).join('');
    pos += [...s.t].length;
    return { ...s, t };
  });
}
