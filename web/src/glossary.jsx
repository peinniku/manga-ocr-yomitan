// Definitions and furigana, laid out like the desktop app.
import { glossaryLines, lineKind } from './gloss-lines.js';

/** A glossary for reading: senses stand out, examples are indented and muted,
 *  and the dictionary's own headword line (redundant with the card) is dropped. */
export function Glossary({ value, limit }) {
  const lines = glossaryLines(value)
    .map((text, i) => ({ text, kind: lineKind(text, i) }))
    .filter((l) => l.kind !== 'headword');
  const shown = limit ? lines.slice(0, limit) : lines;
  return (
    <div class="gloss">
      {shown.map((l, i) => (
        <p key={i} class={`g-${l.kind}`}>
          {l.text}
        </p>
      ))}
      {shown.length < lines.length && <p class="g-more">…</p>}
    </div>
  );
}

/** Sentence with furigana; exactly the characters in [hl0, hl1) are
 *  emphasised (a word with a reading is marked whole). */
export function Ruby({ segments, highlight }) {
  let pos = 0;
  const [a, b] = highlight || [0, 0];
  return (
    <p class="ruby-line">
      {segments.map((s, i) => {
        const chars = [...s.t];
        const start = pos;
        pos += chars.length;
        const from = Math.max(a, start) - start;
        const to = Math.min(b, pos) - start;
        if (s.r) {
          const body = (
            <ruby>
              {s.t}
              <rt>{s.r}</rt>
            </ruby>
          );
          return from < to ? <mark key={i}>{body}</mark> : <span key={i}>{body}</span>;
        }
        if (from >= to) return <span key={i}>{s.t}</span>;
        return (
          <span key={i}>
            {chars.slice(0, from).join('')}
            <mark>{chars.slice(from, to).join('')}</mark>
            {chars.slice(to).join('')}
          </span>
        );
      })}
    </p>
  );
}
