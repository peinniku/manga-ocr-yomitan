// The desktop app's Rust FSRS-7 scheduler, compiled to WebAssembly (crates/srs).
// JSON goes in and out through linear memory, so phone and Mac schedule identically.

let ex = null;

export async function loadSrs(url = '/app/srs.wasm') {
  if (ex) return;
  const bytes = await (await fetch(url)).arrayBuffer();
  const { instance } = await WebAssembly.instantiate(bytes, {});
  ex = instance.exports;
}

/** Testing hook: use an already-instantiated module. */
export function useExports(exports) {
  ex = exports;
}

function call(fn, obj) {
  const input = new TextEncoder().encode(JSON.stringify(obj));
  const ptr = ex.alloc(input.length);
  new Uint8Array(ex.memory.buffer, ptr, input.length).set(input);
  ex[fn](ptr, input.length);
  const out = new Uint8Array(ex.memory.buffer, ex.result_ptr(), ex.result_len());
  const res = JSON.parse(new TextDecoder().decode(out));
  if (res && res.error) throw new Error(res.error);
  return res;
}

/** Schedule derived from all review logs of one card. */
export function replay(logs, config) {
  return call('replay', { config, logs });
}

/** Due times and labels for again / hard / good / easy if answered at `now`. */
export function preview(schedule, now, config) {
  return call('preview', { config, schedule, now });
}
