// ==UserScript==
// @name         Manga OCR → Yomitan Bridge
// @namespace    local.manga-ocr-yomitan
// @version      0.2.1
// @description  Shift-hover an image to OCR (manga-ocr) and inject invisible text so Yomitan picks it up like normal text.
// @match        *://*/*
// @grant        GM_xmlhttpRequest
// @grant        GM_addStyle
// @grant        unsafeWindow
// @connect      127.0.0.1
// @connect      localhost
// @connect      *
// @run-at       document-idle
// ==/UserScript==

(function () {
  'use strict';

  const OCR_ENDPOINT = 'http://127.0.0.1:7331/ocr';
  const MIN_DIM = 200;          // skip icons / avatars
  const STATUS_TIMEOUT = 1500;

  const inflight = new Map();   // src -> Promise<result>
  let lastImg = null;

  // --- styles ----------------------------------------------------------------
  // Use GM_addStyle to bypass strict page CSP (e.g. X uses nonce-based script-src
  // and may also restrict style-src; injecting via <style> can be silently dropped).
  const CSS = `
    .moy-wrap { position: relative; display: inline-block; max-width: 100%; }
    .moy-overlay { position: absolute; inset: 0; pointer-events: none; }
    .moy-block { position: absolute; pointer-events: auto; overflow: hidden; }
    .moy-block p {
      position: absolute;
      margin: 0; padding: 0;
      color: transparent;
      line-height: 1;
      white-space: nowrap;
      user-select: text;
      font-family: "Noto Sans JP", "Hiragino Sans", "Yu Gothic", sans-serif;
    }
    .moy-block.vertical p { writing-mode: vertical-rl; }
    /* Keep glyph color transparent (so the original art is unobstructed) but
       show a subtle gray background under whatever Yomitan currently selects,
       mirroring its normal hover-scan feedback on regular text pages. */
    .moy-block ::selection, .moy-block::selection,
    .moy-block p::selection { background: rgba(128, 128, 128, 0.32) !important; color: transparent !important; }
    .moy-overlay.moy-debug .moy-block { outline: 1px solid #f33; background: rgba(255,80,80,.08); }
    .moy-overlay.moy-debug .moy-block p { outline: 1px solid #3f3; color: rgba(255,40,40,.85); }
    #moy-status {
      position: fixed !important;
      bottom: 12px !important; right: 12px !important;
      background: rgba(0,0,0,.85) !important; color: #fff !important;
      padding: 6px 12px !important; border-radius: 4px !important;
      font: 13px/1.4 system-ui, sans-serif !important;
      z-index: 2147483647 !important; pointer-events: none !important;
      display: block !important; opacity: 1;
      transition: opacity .2s;
    }
    #moy-toggle {
      position: fixed !important;
      bottom: 12px !important; left: 12px !important;
      width: 32px !important; height: 32px !important;
      background: rgba(0,0,0,.78) !important; color: #fff !important;
      border: 1px solid rgba(255,255,255,.3) !important; border-radius: 50% !important;
      font: 16px/1 system-ui, sans-serif !important;
      display: flex !important; align-items: center !important; justify-content: center !important;
      cursor: pointer !important;
      z-index: 2147483647 !important;
      user-select: none !important;
      opacity: .55; transition: opacity .15s;
    }
    #moy-toggle:hover { opacity: 1 !important; }
    #moy-toggle.on { background: #d33 !important; opacity: 1; }
  `;
  if (typeof GM_addStyle === 'function') {
    GM_addStyle(CSS);
  } else {
    const style = document.createElement('style');
    style.textContent = CSS;
    document.documentElement.appendChild(style);
  }

  // --- status indicator ------------------------------------------------------
  let statusEl, statusTimer;
  function status(text) {
    if (!statusEl) {
      statusEl = document.createElement('div');
      statusEl.id = 'moy-status';
      document.documentElement.appendChild(statusEl);
    }
    statusEl.textContent = text;
    statusEl.style.opacity = '1';
    clearTimeout(statusTimer);
    if (!text) statusEl.style.opacity = '0';
    else statusTimer = setTimeout(() => (statusEl.style.opacity = '0'), STATUS_TIMEOUT);
  }

  // --- network ---------------------------------------------------------------
  function gm(method, url, opts = {}) {
    return new Promise((resolve, reject) => {
      GM_xmlhttpRequest({
        method,
        url,
        responseType: opts.responseType || 'text',
        headers: opts.headers || {},
        data: opts.body,
        onload: r => (r.status >= 200 && r.status < 300 ? resolve(r) : reject(new Error(`${r.status} ${r.statusText}`))),
        onerror: () => reject(new Error('network error')),
        ontimeout: () => reject(new Error('timeout')),
      });
    });
  }

  async function imageToDataUrl(src) {
    const r = await gm('GET', src, { responseType: 'blob' });
    return await new Promise((resolve, reject) => {
      const fr = new FileReader();
      fr.onload = () => resolve(fr.result);
      fr.onerror = reject;
      fr.readAsDataURL(r.response);
    });
  }

  async function ocr(src) {
    if (inflight.has(src)) return inflight.get(src);
    const p = (async () => {
      const dataUrl = await imageToDataUrl(src);
      const r = await gm('POST', OCR_ENDPOINT, {
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ image_data: dataUrl, cache_key: src }),
        responseType: 'json',
      });
      return typeof r.response === 'string' ? JSON.parse(r.response) : r.response;
    })();
    inflight.set(src, p);
    p.catch(() => inflight.delete(src));
    return p;
  }

  // --- DOM injection ---------------------------------------------------------
  // Find a top-layer-friendly host for the overlay: if the img is inside an
  // open <dialog>, append there so we share the same top-layer stacking context.
  function pickOverlayHost(img) {
    let cur = img;
    while (cur) {
      if (cur instanceof HTMLDialogElement && cur.open) return cur;
      cur = cur.parentNode;
      if (cur && cur.host) cur = cur.host;  // shadow root
    }
    return document.body;
  }

  function renderOverlay(img, result) {
    if (img._moyOverlay && document.contains(img._moyOverlay)) return;

    const overlay = document.createElement('div');
    overlay.className = 'moy-overlay' + (debugOn ? ' moy-debug' : '');
    overlay.style.position = 'fixed';
    overlay.style.pointerEvents = 'none';
    overlay.style.left = '0px';
    overlay.style.top = '0px';
    overlay.style.width = '0px';
    overlay.style.height = '0px';

    const W = result.width, H = result.height;
    const pct = (v, total) => (100 * v / total) + '%';

    for (const b of result.blocks) {
      const [bx1, by1, bx2, by2] = b.box;
      const bw = bx2 - bx1, bh = by2 - by1;
      if (bw <= 0 || bh <= 0) continue;

      const blk = document.createElement('div');
      blk.className = 'moy-block' + (b.vertical ? ' vertical' : '');
      blk.style.left   = pct(bx1, W);
      blk.style.top    = pct(by1, H);
      blk.style.width  = pct(bw, W);
      blk.style.height = pct(bh, H);

      const lines = b.lines || [];
      const coords = b.lines_coords || [];

      for (let i = 0; i < lines.length; i++) {
        const text = lines[i];
        if (!text) continue;
        const c = coords[i];
        let lx1, ly1, lx2, ly2;
        if (c && c.length >= 4 && Array.isArray(c[0])) {
          const xs = c.map(p => p[0]);
          const ys = c.map(p => p[1]);
          lx1 = Math.min(...xs); ly1 = Math.min(...ys);
          lx2 = Math.max(...xs); ly2 = Math.max(...ys);
        } else if (b.vertical) {
          const colW = bw / lines.length;
          lx1 = bx1 + (lines.length - 1 - i) * colW;
          lx2 = lx1 + colW;
          ly1 = by1; ly2 = by2;
        } else {
          const rowH = bh / lines.length;
          lx1 = bx1; lx2 = bx2;
          ly1 = by1 + i * rowH; ly2 = ly1 + rowH;
        }
        const lw = lx2 - lx1, lh = ly2 - ly1;

        const p = document.createElement('p');
        p.textContent = text;
        p.style.left   = pct(lx1 - bx1, bw);
        p.style.top    = pct(ly1 - by1, bh);
        p.style.width  = pct(lw, bw);
        p.style.height = pct(lh, bh);
        p.dataset.fontRatio = (b.vertical ? lw : lh) / H;
        blk.appendChild(p);
      }
      overlay.appendChild(blk);
    }

    pickOverlayHost(img).appendChild(overlay);
    img._moyOverlay = overlay;

    let last = '';
    function reposition() {
      if (!document.contains(img)) {
        cleanup();
        return;
      }
      const r = img.getBoundingClientRect();
      if (r.width < 4 || r.height < 4) {
        if (overlay.style.display !== 'none') overlay.style.display = 'none';
        return;
      }
      overlay.style.display = '';
      const key = `${r.left}|${r.top}|${r.width}|${r.height}`;
      if (key === last) return;
      last = key;
      overlay.style.left   = r.left + 'px';
      overlay.style.top    = r.top + 'px';
      overlay.style.width  = r.width + 'px';
      overlay.style.height = r.height + 'px';
      for (const p of overlay.querySelectorAll('p[data-font-ratio]')) {
        p.style.fontSize = (r.height * parseFloat(p.dataset.fontRatio)) + 'px';
      }
    }

    const ro = new ResizeObserver(reposition);
    ro.observe(img);
    window.addEventListener('scroll', reposition, true);
    window.addEventListener('resize', reposition);

    // Animations / SPA churn: poll briefly after attach.
    let n = 0;
    const poll = setInterval(() => {
      reposition();
      if (++n > 30 || !document.contains(img)) clearInterval(poll);
    }, 100);

    function cleanup() {
      overlay.remove();
      delete img._moyOverlay;
      ro.disconnect();
      clearInterval(poll);
      window.removeEventListener('scroll', reposition, true);
      window.removeEventListener('resize', reposition);
    }

    if (img.complete && img.clientWidth) reposition();
    else img.addEventListener('load', reposition, { once: true });
  }

  // Floating debug toggle button (shows up on every page where the script runs).
  let toggleBtn;
  let debugOn = false;
  function applyDebug() {
    document.querySelectorAll('.moy-overlay').forEach(o => o.classList.toggle('moy-debug', debugOn));
    if (toggleBtn) toggleBtn.classList.toggle('on', debugOn);
  }
  function ensureToggleBtn() {
    if (toggleBtn) return;
    toggleBtn = document.createElement('div');
    toggleBtn.id = 'moy-toggle';
    toggleBtn.textContent = '👁';
    toggleBtn.title = 'Toggle OCR overlay (debug)';
    toggleBtn.addEventListener('click', () => {
      debugOn = !debugOn;
      applyDebug();
      const n = document.querySelectorAll('.moy-overlay').length;
      status(`debug ${debugOn ? 'ON' : 'OFF'} (${n} overlay${n === 1 ? '' : 's'})`);
    }, true);
    document.documentElement.appendChild(toggleBtn);
  }
  ensureToggleBtn();

  // --- trigger ---------------------------------------------------------------
  async function processImg(img) {
    if (!img || !(img instanceof HTMLImageElement)) return;
    if (img.dataset.moyState === 'done' || img.dataset.moyState === 'loading') return;
    if (img.naturalWidth < MIN_DIM && img.naturalHeight < MIN_DIM) return;

    const src = img.currentSrc || img.src;
    if (!src) return;

    img.dataset.moyState = 'loading';
    status('OCR…');
    console.log('[moy] start OCR:', src.slice(0, 120), 'natural', img.naturalWidth, 'x', img.naturalHeight);
    try {
      const result = await ocr(src);
      const n = (result.blocks || []).length;
      console.log('[moy] OCR result:', n, 'blocks', result);
      renderOverlay(img, result);
      img.dataset.moyState = 'done';
      const overlayEl = img._moyOverlay;
      console.log('[moy] overlay rendered:', overlayEl, 'blocks:', overlayEl ? overlayEl.children.length : 0,
        'rect:', overlayEl ? overlayEl.getBoundingClientRect() : null);
      status(n ? `OCR ✓ (${n} blocks)` : 'OCR: no text found');
    } catch (e) {
      img.dataset.moyState = '';  // allow retry
      console.warn('[moy] OCR failed:', e);
      status('OCR failed — see console');
    }
  }

  document.addEventListener('mouseover', e => {
    if (e.target instanceof HTMLImageElement) {
      lastImg = e.target;
      if (e.shiftKey) processImg(e.target);
    }
  }, true);

  document.addEventListener('keydown', e => {
    if (e.key === 'Shift' && !e.repeat && lastImg) processImg(lastImg);
  }, true);

  document.addEventListener('mouseout', e => {
    if (e.target === lastImg) lastImg = null;
  }, true);
})();
