// Yomitan glossaries (plain strings and structured-content trees) flattened
// into display lines and classified, exactly like the desktop app
// (crates/app/src/glossary.rs), so a definition reads the same on both.

const BLOCK = new Set(['div', 'p', 'li', 'ol', 'ul', 'table', 'tr', 'details', 'summary', 'thead', 'tbody', 'dl']);

function walk(v, st) {
  if (v == null) return;
  if (typeof v === 'string') {
    st.buf += v;
    return;
  }
  if (Array.isArray(v)) {
    for (const it of v) walk(it, st);
    return;
  }
  if (typeof v !== 'object') return;
  if (v.type === 'text') {
    st.buf += v.text || '';
    return;
  }
  if (v.type === 'structured-content') {
    walk(v.content, st);
    return;
  }
  if (v.type === 'image') return;
  const tag = v.tag || '';
  if (tag === 'br') return flush(st);
  if (tag === 'img' || tag === 'rp') return;
  if (tag === 'rt') {
    const inner = { buf: '', lines: st.lines };
    walk(v.content, inner);
    if (inner.buf) st.buf += `（${inner.buf}）`;
    return;
  }
  const block = BLOCK.has(tag);
  if (block) flush(st);
  if (tag === 'li') st.buf += '・';
  walk(v.content, st);
  if (block) flush(st);
  else if (tag === 'td' || tag === 'th') st.buf += ' ';
}

function flush(st) {
  const line = st.buf.trim();
  if (line) st.lines.push(line);
  st.buf = '';
}

/** Display lines for one entry's glossary. */
export function glossaryLines(glossary) {
  const st = { buf: '', lines: [] };
  for (const item of Array.isArray(glossary) ? glossary : [glossary]) {
    walk(item, st);
    flush(st);
  }
  return st.lines;
}

const inRange = (c, a, b) => c >= a && c <= b;

/** headword | sense | example | note | text */
export function lineKind(line, index) {
  const first = line[0] || ' ';
  const n = [...line].length;
  if (index === 0 && (first === '＊' || first === '*' || line.includes('【')) && n < 40) return 'headword';
  if (first === '「' || first === '『') return 'example';
  const cp = first.codePointAt(0);
  if (inRange(cp, 0x3280, 0x3289) || inRange(cp, 0x2460, 0x2473) || inRange(cp, 0x2776, 0x277f) || inRange(cp, 0x2474, 0x2487) || inRange(cp, 0x24b6, 0x24e9)) {
    return 'sense';
  }
  if ('（(〈〔［'.includes(first) && n < 24) return 'note';
  return 'text';
}
