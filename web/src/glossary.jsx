// Render Yomitan structured content safely as Preact elements (whitelisted tags, no innerHTML).
import { h } from 'preact';

const TAGS = new Set(['span', 'div', 'ruby', 'rt', 'rp', 'table', 'thead', 'tbody', 'tfoot', 'tr', 'td', 'th', 'ol', 'ul', 'li', 'details', 'summary', 'br', 'b', 'i', 'em', 'strong', 'sup', 'sub']);
const STYLE = { fontWeight: 1, fontStyle: 1, fontSize: 1, textDecorationLine: 1, verticalAlign: 1, textAlign: 1, marginLeft: 1, marginTop: 1, listStyleType: 1 };

function node(n, key) {
  if (n == null) return null;
  if (typeof n === 'string') return n;
  if (Array.isArray(n)) return n.map((c, i) => node(c, i));
  if (typeof n !== 'object') return null;
  if (n.type === 'text') return n.text || '';
  if (n.type === 'structured-content') return node(n.content, key);
  if (n.type === 'image' || n.tag === 'img') return null;
  const tag = TAGS.has(n.tag) ? n.tag : 'span';
  const style = {};
  if (n.style && typeof n.style === 'object') {
    for (const [k, v] of Object.entries(n.style)) if (STYLE[k] && typeof v === 'string') style[k] = v;
  }
  return h(tag, { key, style }, node(n.content));
}

export function Glossary({ value }) {
  const items = Array.isArray(value) ? value : [value];
  return (
    <div class="gloss">
      {items.map((it, i) => (
        <div key={i}>{node(it, i)}</div>
      ))}
    </div>
  );
}

/** Sentence with furigana; segments overlapping [hl0, hl1) are emphasised. */
export function Ruby({ segments, highlight }) {
  let pos = 0;
  return (
    <p class="ruby-line">
      {segments.map((s, i) => {
        const len = [...s.t].length;
        const on = highlight && pos < highlight[1] && pos + len > highlight[0];
        pos += len;
        const body = s.r ? (
          <ruby>
            {s.t}
            <rt>{s.r}</rt>
          </ruby>
        ) : (
          s.t
        );
        return on ? <mark key={i}>{body}</mark> : <span key={i}>{body}</span>;
      })}
    </p>
  );
}
