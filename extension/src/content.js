// pic_yomi content script.
//  - Shift + hover an image  → OCR it (desktop app), keep an invisible per-character layer over it.
//  - Shift + hover OCR'd text or any page text → dictionary popup with Chinese explanation and "add to word list".
(() => {
  'use strict';
  if (window.__picyomi) return;
  window.__picyomi = true;

  const api = typeof browser !== 'undefined' ? browser : chrome;
  const MIN_DIM = 200;
  const MAX_SCAN = 20;

  function send(type, payload) {
    return new Promise((resolve, reject) => {
      api.runtime.sendMessage({ type, payload }, (res) => {
        const err = api.runtime.lastError;
        if (err) return reject(new Error(err.message));
        if (!res) return reject(new Error('no response'));
        res.ok ? resolve(res.data) : reject(new Error(res.error));
      });
    });
  }

  // ---------------------------------------------------------------- styles
  // Overlay elements live in the page, so they are styled through the CSSOM
  // (allowed under strict CSP). The popup lives in a shadow root.
  function css(el, styles) {
    for (const [k, v] of Object.entries(styles)) el.style.setProperty(k, v, 'important');
    return el;
  }

  // ---------------------------------------------------------------- status toast
  let toast, toastTimer;
  function status(text, ms = 1600) {
    if (!toast) {
      toast = css(document.createElement('div'), {
        position: 'fixed', right: '12px', bottom: '12px', 'z-index': '2147483647',
        background: 'rgba(20,20,20,.88)', color: '#fff', padding: '6px 12px', 'border-radius': '6px',
        font: '13px/1.4 system-ui, sans-serif', 'pointer-events': 'none', transition: 'opacity .2s',
      });
      document.documentElement.appendChild(toast);
    }
    toast.textContent = text;
    css(toast, { opacity: '1' });
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => css(toast, { opacity: '0' }), ms);
  }

  // ---------------------------------------------------------------- OCR overlays
  /** img -> { key, page, layer, cleanup } */
  const overlays = new Map();
  const pending = new WeakSet();
  let showText = false;
  let lastImg = null;

  function imgSrc(img) {
    return img.currentSrc || img.src || '';
  }

  function eligible(img) {
    return img instanceof HTMLImageElement && (img.naturalWidth >= MIN_DIM || img.naturalHeight >= MIN_DIM) && imgSrc(img);
  }

  async function ocrImage(img) {
    if (!eligible(img) || pending.has(img)) return;
    const existing = overlays.get(img);
    if (existing && existing.src === imgSrc(img)) return;
    if (existing) existing.cleanup();
    pending.add(img);
    status('识别中…', 20000);
    try {
      const src = imgSrc(img);
      const payload = src.startsWith('data:') ? { src: `data:${src.length}:${src.slice(-64)}`, dataUrl: src } : { src };
      const { key, page } = await send('ocr', payload);
      const n = page.blocks.length;
      attachOverlay(img, src, key, page);
      status(n ? `识别到 ${n} 段文字` : '这张图里没有识别到日语文字');
    } catch (e) {
      status(`识别失败：${e.message}`, 4000);
    } finally {
      pending.delete(img);
    }
  }

  /** Overlay root: fixed-position box that follows the image. */
  function attachOverlay(img, src, key, page) {
    const layer = css(document.createElement('div'), {
      position: 'fixed', left: '0', top: '0', width: '0', height: '0', 'pointer-events': 'none', 'z-index': '2147483646',
    });
    const glyphs = [];
    page.blocks.forEach((b, bi) => {
      let off = 0;
      for (const ch of b.chars) {
        const g = css(document.createElement('span'), {
          position: 'absolute', 'line-height': '1', color: 'transparent', 'white-space': 'nowrap',
          'font-family': '"Hiragino Sans","Noto Sans JP",sans-serif', 'text-align': 'center',
        });
        g.textContent = ch.c;
        g.dataset.block = String(bi);
        g.dataset.offset = String(off);
        off += [...ch.c].length;
        glyphs.push({ el: g, box: ch.box });
        layer.appendChild(g);
      }
    });
    (img.closest('dialog[open]') || document.body).appendChild(layer);

    let lastKey = '';
    function place() {
      if (!document.contains(img)) return cleanup();
      const r = img.getBoundingClientRect();
      const k = `${r.left}|${r.top}|${r.width}|${r.height}|${showText}`;
      if (k === lastKey) return;
      lastKey = k;
      css(layer, { left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px`, display: r.width < 4 ? 'none' : 'block' });
      const sx = r.width / page.width, sy = r.height / page.height;
      for (const { el, box } of glyphs) {
        const w = (box[2] - box[0]) * sx, h = (box[3] - box[1]) * sy;
        css(el, {
          left: `${box[0] * sx}px`, top: `${box[1] * sy}px`, width: `${w}px`, height: `${h}px`,
          'font-size': `${Math.max(6, Math.min(w, h) * 0.95)}px`,
          color: showText ? 'rgba(220,30,30,.9)' : 'transparent',
          background: showText ? 'rgba(255,255,255,.75)' : 'transparent',
        });
      }
    }
    const ro = new ResizeObserver(place);
    ro.observe(img);
    const onScroll = () => place();
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('resize', onScroll);
    let n = 0;
    const poll = setInterval(() => { place(); if (++n > 40) clearInterval(poll); }, 100);
    function cleanup() {
      ro.disconnect();
      clearInterval(poll);
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', onScroll);
      layer.remove();
      overlays.delete(img);
    }
    overlays.set(img, { src, key, page, layer, place, cleanup, img });
    place();
  }

  function refreshOverlays() {
    for (const o of overlays.values()) o.place();
  }

  /** Map a viewport point to (overlay, block index, char offset) if it is on OCR'd text. */
  function hitOcr(x, y) {
    for (const o of overlays.values()) {
      const r = o.img.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
      const px = ((x - r.left) / r.width) * o.page.width;
      const py = ((y - r.top) / r.height) * o.page.height;
      let best = null;
      o.page.blocks.forEach((b, bi) => {
        let off = 0;
        for (const ch of b.chars) {
          const [x0, y0, x1, y1] = ch.box;
          const pad = Math.max(x1 - x0, y1 - y0) * 0.15;
          const dx = Math.max(x0 - pad - px, 0, px - x1 - pad);
          const dy = Math.max(y0 - pad - py, 0, py - y1 - pad);
          const d = Math.hypot(dx, dy);
          if (!best || d < best.d) best = { d, bi, off };
          off += [...ch.c].length;
        }
      });
      if (best && best.d === 0) return { overlay: o, block: best.bi, offset: best.off };
    }
    return null;
  }

  // ---------------------------------------------------------------- page text
  function caretAt(x, y) {
    if (document.caretPositionFromPoint) {
      const p = document.caretPositionFromPoint(x, y);
      return p && { node: p.offsetNode, offset: p.offset };
    }
    if (document.caretRangeFromPoint) {
      const r = document.caretRangeFromPoint(x, y);
      return r && { node: r.startContainer, offset: r.startOffset };
    }
    return null;
  }

  const SENTENCE_END = /[。！？!?\n]/;
  const JP = /[぀-ヿ㐀-鿿ｦ-ﾟ々〆ヵヶー]/;

  /** Sentence around the caret within its text node, with the caret's character offset in it. */
  function textAt(x, y) {
    const c = caretAt(x, y);
    if (!c || !c.node || c.node.nodeType !== Node.TEXT_NODE) return null;
    const text = c.node.textContent;
    let i = c.offset;
    // caret positions can land just after the glyph under the pointer
    if (i >= text.length || !JP.test(text[i])) {
      if (i > 0 && JP.test(text[i - 1])) i -= 1;
      else return null;
    }
    // make sure the pointer is really over that glyph
    const range = document.createRange();
    range.setStart(c.node, i);
    range.setEnd(c.node, i + 1);
    const rect = range.getBoundingClientRect();
    if (x < rect.left - 1 || x > rect.right + 1 || y < rect.top - 1 || y > rect.bottom + 1) return null;
    let s = i;
    while (s > 0 && !SENTENCE_END.test(text[s - 1]) && i - s < 120) s--;
    let e = i;
    while (e < text.length && !SENTENCE_END.test(text[e]) && e - i < 120) e++;
    if (e < text.length) e++;
    const raw = text.slice(s, e);
    const base = s + (raw.length - raw.trimStart().length);
    return { node: c.node, base, sentence: raw.trim(), offset: [...text.slice(base, i)].length };
  }

  // ---------------------------------------------------------------- highlight
  const marks = [];
  function clearMarks() {
    while (marks.length) marks.pop().remove();
  }
  function mark(rect) {
    const m = css(document.createElement('div'), {
      position: 'fixed', left: `${rect.left}px`, top: `${rect.top}px`, width: `${rect.width}px`, height: `${rect.height}px`,
      background: 'rgba(255,196,0,.35)', 'border-radius': '2px', 'pointer-events': 'none', 'z-index': '2147483646',
    });
    document.documentElement.appendChild(m);
    marks.push(m);
  }
  function markOcr(hit, len) {
    clearMarks();
    const o = hit.overlay, b = o.page.blocks[hit.block];
    const r = o.img.getBoundingClientRect();
    const sx = r.width / o.page.width, sy = r.height / o.page.height;
    let off = 0;
    for (const ch of b.chars) {
      if (off >= hit.offset && off < hit.offset + len) {
        const [x0, y0, x1, y1] = ch.box;
        mark({ left: r.left + x0 * sx, top: r.top + y0 * sy, width: (x1 - x0) * sx, height: (y1 - y0) * sy });
      }
      off += [...ch.c].length;
    }
  }
  function markText(t, len) {
    clearMarks();
    const text = t.node.textContent;
    // code-point offsets in the sentence -> UTF-16 offsets in the text node
    const startIdx = t.base + [...t.sentence].slice(0, t.offset).join('').length;
    const endIdx = startIdx + [...t.sentence].slice(t.offset, t.offset + len).join('').length;
    const range = document.createRange();
    range.setStart(t.node, Math.min(startIdx, text.length));
    range.setEnd(t.node, Math.min(endIdx, text.length));
    for (const rect of range.getClientRects()) mark(rect);
  }

  // ---------------------------------------------------------------- popup (shadow DOM)
  let host, root, popup, current = null;
  const STYLE = `
    :host { all: initial; }
    .pop { position: fixed; z-index: 2147483647; width: 400px; max-height: 460px; overflow: auto; box-sizing: border-box;
      background: #fff; color: #1d1d1f; border: 1px solid #d0d0d6; border-radius: 10px; box-shadow: 0 12px 32px rgba(0,0,0,.18);
      font: 14px/1.55 "Hiragino Sans","Noto Sans JP","PingFang SC",system-ui,sans-serif; padding: 12px 14px; }
    @media (prefers-color-scheme: dark) { .pop { background: #1f1f22; color: #ececf0; border-color: #3a3a40; } .zh, .tab.on { background: #2b2b31 !important; } .muted { color: #9a9aa3 !important; } }
    .head { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
    .word { font-size: 22px; font-weight: 700; }
    .muted { color: #575761; font-size: 12px; }
    .tabs { display: flex; flex-wrap: wrap; gap: 4px; margin: 6px 0; }
    .tab { border: 1px solid #d0d0d6; border-radius: 6px; padding: 1px 7px; font-size: 12px; cursor: pointer; background: transparent; color: inherit; }
    .tab.on { background: #eef2ff; border-color: #6b7cff; }
    .zh { background: #f4f5f8; border-radius: 8px; padding: 8px 10px; margin: 8px 0; font-size: 13px; }
    .zh .ex { margin-top: 4px; }
    .ask { margin: 8px 0 2px; border: 1px solid #d0d0d6; border-radius: 6px; padding: 3px 10px; font-size: 12px; cursor: pointer; background: transparent; color: inherit; }
    .defs { font-size: 13px; }
    .defs ul, .defs ol { margin: 2px 0; padding-left: 18px; }
    .defs a { color: #3b5bdb; cursor: pointer; text-decoration: none; }
    .say { border: 0; background: transparent; color: inherit; opacity: .6; cursor: pointer; padding: 2px; line-height: 0; border-radius: 5px; align-self: center; }
    .say:hover { opacity: 1; background: rgba(127,127,127,.15); }
    .say.busy { opacity: .3; }
    .say svg { width: 16px; height: 16px; }
    .row { display: flex; gap: 6px; align-items: center; }
    .add { margin-left: auto; border: 0; border-radius: 6px; padding: 4px 10px; font-size: 12px; cursor: pointer; background: #3b5bdb; color: #fff; }
    .add[disabled] { background: #9aa3b8; cursor: default; }
    ruby rt { font-size: 9px; }
  `;

  function ensurePopup() {
    if (host) return;
    host = document.createElement('picyomi-popup');
    root = host.attachShadow({ mode: 'closed' });
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(STYLE);
    root.adoptedStyleSheets = [sheet];
    popup = document.createElement('div');
    popup.className = 'pop';
    root.appendChild(popup);
    document.documentElement.appendChild(host);
  }

  function hidePopup() {
    if (popup) popup.style.display = 'none';
    clearMarks();
    current = null;
  }

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  const SC_TAGS = new Set(['span', 'div', 'ruby', 'rt', 'rp', 'table', 'thead', 'tbody', 'tfoot', 'tr', 'td', 'th', 'ol', 'ul', 'li', 'details', 'summary', 'br', 'a', 'b', 'i', 'em', 'strong', 'sup', 'sub']);
  const SC_STYLE = { fontWeight: 'font-weight', fontStyle: 'font-style', fontSize: 'font-size', textDecorationLine: 'text-decoration-line', verticalAlign: 'vertical-align', textAlign: 'text-align', marginLeft: 'margin-left', marginTop: 'margin-top', listStyleType: 'list-style-type' };

  /** Render Yomitan structured content safely (whitelisted tags, no innerHTML). */
  function renderSC(node, parent) {
    if (node == null) return;
    if (typeof node === 'string') return parent.appendChild(document.createTextNode(node));
    if (Array.isArray(node)) return node.forEach((n) => renderSC(n, parent));
    if (typeof node !== 'object') return;
    if (node.type === 'text') return parent.appendChild(document.createTextNode(node.text || ''));
    if (node.type === 'structured-content') return renderSC(node.content, parent);
    if (node.type === 'image' || node.tag === 'img') return;
    const tag = SC_TAGS.has(node.tag) ? node.tag : 'span';
    const e = document.createElement(tag === 'a' ? 'a' : tag);
    if (node.style && typeof node.style === 'object') {
      for (const [k, v] of Object.entries(node.style)) if (SC_STYLE[k] && typeof v === 'string') e.style.setProperty(SC_STYLE[k], v);
    }
    if (tag === 'a' && typeof node.href === 'string') {
      const m = node.href.match(/[?&]query=([^&]+)/);
      if (m) e.addEventListener('click', () => lookupWord(decodeURIComponent(m[1])));
    }
    renderSC(node.content, e);
    parent.appendChild(e);
  }

  // ---------------------------------------------------------------- pronunciation
  // Web Audio, not <audio>: a decoded buffer needs no URL, so page CSP can't block it.
  let audioCtx = null, playing = null;
  const clips = new Map();

  function speakerIcon() {
    const NS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('fill', 'none');
    svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '2');
    svg.setAttribute('stroke-linecap', 'round');
    svg.setAttribute('stroke-linejoin', 'round');
    for (const d of ['M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5z', 'M16 9a5 5 0 0 1 0 6', 'M19.4 18.4a9 9 0 0 0 0-12.8']) {
      const p = document.createElementNS(NS, 'path');
      p.setAttribute('d', d);
      svg.append(p);
    }
    return svg;
  }

  async function play(req, btn) {
    const key = `${req.s || ''}|${req.e}|${req.r}`;
    btn?.classList.add('busy');
    try {
      audioCtx = audioCtx || new AudioContext();
      if (audioCtx.state === 'suspended') await audioCtx.resume();
      let buf = clips.get(key);
      if (!buf) {
        const a = await send('audio', req);
        const bin = atob(a.data);
        const bytes = new Uint8Array(bin.length);
        for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
        buf = await audioCtx.decodeAudioData(bytes.buffer);
        clips.set(key, buf);
      }
      if (playing) try { playing.stop(); } catch (_) {}
      const src = audioCtx.createBufferSource();
      src.buffer = buf;
      src.connect(audioCtx.destination);
      src.start();
      playing = src;
    } catch (err) {
      status(`发音失败：${err.message}`, 3000);
    } finally {
      btn?.classList.remove('busy');
    }
  }

  function sayButton(title, req) {
    const b = el('button', 'say');
    b.title = title;
    b.append(speakerIcon());
    b.addEventListener('click', (ev) => { ev.stopPropagation(); play(req, b); });
    return b;
  }

  function renderPopup(state) {
    ensurePopup();
    popup.replaceChildren();
    const { res, sel } = state;
    const entries = res.entries;
    const e = entries[sel];
    const freq = (e.frequencies || []).map((f) => f.display).filter(Boolean).join(' · ');

    const head = el('div', 'head');
    head.append(el('span', 'word', e.expression), el('span', 'muted', e.reading), sayButton('发音', { e: e.expression, r: e.reading }));
    if (freq) head.append(el('span', 'muted', `频率 ${freq}`));
    if (e.reasons && e.reasons.length) head.append(el('span', 'muted', e.reasons.join(' ← ')));
    const saved = res.saved && res.saved[sel];
    const add = el('button', 'add', saved ? '✓ 已在词单' : '＋ 加入词单');
    add.disabled = !!saved;
    add.addEventListener('click', () => addCard(state, add));
    head.append(add);
    popup.append(head);

    if (entries.length > 1) {
      const tabs = el('div', 'tabs');
      entries.forEach((x, i) => {
        const t = el('button', `tab${i === sel ? ' on' : ''}`, `${x.expression}【${x.reading}】`);
        t.addEventListener('click', () => { state.sel = i; state.zh = undefined; renderPopup(state); });
        tabs.append(t);
      });
      popup.append(tabs);
    }

    const defs = el('div', 'defs');
    defs.append(el('div', 'muted', e.dict));
    renderSC(e.glossary, defs);
    popup.append(defs);

    // Chinese explanation only on request; adding to the word list generates one for the card.
    const sentenceSay = state.sentence && state.sentence.trim() !== e.expression
      ? sayButton('朗读整句', { s: state.sentence, e: e.expression, r: e.reading }) : null;
    if (state.zh === undefined) {
      const row = el('div', 'row');
      const ask = el('button', 'ask', '中文解释');
      ask.addEventListener('click', () => explain(state));
      row.append(ask);
      if (sentenceSay) row.append(sentenceSay);
      popup.append(row);
    } else {
      if (sentenceSay) { const row = el('div', 'row'); row.append(el('span', 'muted', '整句'), sentenceSay); popup.append(row); }
      const zh = el('div', 'zh');
      if (state.zh === null) zh.textContent = '中文解释生成中…';
      else if (state.zh.error) zh.textContent = `中文解释不可用：${state.zh.error}`;
      else {
        const z = state.zh;
        if (z.meaning_zh) zh.append(el('div', null, z.meaning_zh));
        if (z.gloss_zh) zh.append(el('div', 'muted', `义项：${z.gloss_zh}`));
        if (z.sentence_zh) zh.append(el('div', null, `译：${z.sentence_zh}`));
        for (const ex of z.examples || []) {
          const d = el('div', 'ex');
          d.append(el('div', null, ex.ja), el('div', 'muted', ex.zh));
          zh.append(d);
        }
      }
      popup.append(zh);
    }
    popup.style.display = 'block';
  }

  function placePopup(x, y) {
    ensurePopup();
    // Reserve the maximum height: the Chinese explanation arrives later and grows the popup.
    const w = 400, h = Math.min(460, innerHeight - 16);
    let left = x + 16, top = y + 18;
    if (left + w > innerWidth - 8) left = Math.max(8, x - w - 16);
    if (top + h > innerHeight - 8) top = Math.max(8, innerHeight - h - 8);
    popup.style.left = `${left}px`;
    popup.style.top = `${top}px`;
  }

  async function explain(state) {
    const e = state.res.entries[state.sel];
    state.zh = null;
    renderPopup(state);
    try {
      const z = await send('explain', { expression: e.expression, reading: e.reading, sentence: state.sentence, definition_hint: '' });
      if (current === state) {
        // The model read the sentence and chose another reading: show that entry.
        const j = z.reading ? state.res.entries.findIndex((x) => x.expression === e.expression && x.reading === z.reading) : -1;
        if (j >= 0) state.sel = j;
        state.zh = z;
        renderPopup(state);
      }
    } catch (err) {
      if (current === state) {
        // Not configured: hide silently; other failures are shown.
        state.zh = { error: /not configured/.test(err.message) ? '还没配置（pic_yomi → 设置 → 中文解释）' : err.message };
        renderPopup(state);
      }
    }
  }

  async function addCard(state, btn) {
    const e = state.res.entries[state.sel];
    btn.disabled = true;
    btn.textContent = '加入中…';
    try {
      const card = await send('addCard', {
        expression: e.expression, reading: e.reading, sentence: state.sentence,
        source_url: location.href, source_title: document.title,
        image_key: state.imageKey || null, crop: state.crop || null,
        explanation: state.zh && !state.zh.error ? state.zh : null,
        frequency: (e.frequencies || [])[0]?.display || null,
      });
      state.res.saved[state.sel] = card.id;
      renderPopup(state);
      status(`已加入词单：${card.expression}`);
    } catch (err) {
      btn.disabled = false;
      btn.textContent = '＋ 加入词单';
      status(`加入失败：${err.message}`, 4000);
    }
  }

  async function lookupAt(sentence, offset, x, y, extra, onMatch) {
    const sig = `${sentence}|${offset}`;
    if (current && current.sig === sig) return;
    const state = { sig, sentence, offset, sel: 0, zh: undefined, ...extra };
    current = state;
    let res;
    try {
      res = await send('lookup', { text: sentence, offset });
    } catch (err) {
      status(err.message, 4000);
      return;
    }
    if (current !== state) return;
    if (!res.entries.length) { hidePopup(); current = { sig }; return; }
    state.res = res;
    onMatch(res.match_length);
    renderPopup(state);
    placePopup(x, y);
  }

  function lookupWord(word) {
    if (!current) return;
    const { x, y } = lastPointer;
    lookupAt(word, 0, x, y, {}, () => clearMarks());
  }

  // ---------------------------------------------------------------- input
  const lastPointer = { x: 0, y: 0 };
  let scheduled = false;

  function scan(x, y) {
    const hit = hitOcr(x, y);
    if (hit) {
      const b = hit.overlay.page.blocks[hit.block];
      return lookupAt(b.text, hit.offset, x, y, { imageKey: hit.overlay.key, crop: b.bubble || b.box }, (len) => markOcr(hit, len));
    }
    const t = textAt(x, y);
    if (t && t.sentence) return lookupAt(t.sentence, t.offset, x, y, {}, (len) => markText(t, len));
  }

  document.addEventListener('mousemove', (e) => {
    lastPointer.x = e.clientX;
    lastPointer.y = e.clientY;
    if (e.target instanceof HTMLImageElement) lastImg = e.target;
    if (!e.shiftKey || scheduled) return;
    if (host && e.composedPath().includes(host)) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      if (e.target instanceof HTMLImageElement && !overlays.has(e.target)) ocrImage(e.target);
      else scan(lastPointer.x, lastPointer.y);
    });
  }, true);

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Shift' && !e.repeat) {
      const t = document.elementFromPoint(lastPointer.x, lastPointer.y);
      const img = t instanceof HTMLImageElement ? t : lastImg && lastImg.matches(':hover') ? lastImg : null;
      if (img && !overlays.has(img)) ocrImage(img);
      else scan(lastPointer.x, lastPointer.y);
    } else if (e.key === 'Escape') {
      hidePopup();
    }
  }, true);

  document.addEventListener('mousedown', (e) => {
    if (host && e.composedPath().includes(host)) return;
    hidePopup();
  }, true);

  api.runtime.onMessage.addListener((msg) => {
    if (msg && msg.type === 'toggleText') {
      showText = !showText;
      refreshOverlays();
      status(showText ? '显示识别文字' : '隐藏识别文字');
    }
  });
})();
