// Draw the app icon (speech bubble on indigo) as PNGs without image libraries.
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

const out = join(dirname(fileURLToPath(import.meta.url)), '../public');

function crc32(buf) {
  let c, crc = 0xffffffff;
  for (let n = 0; n < buf.length; n++) {
    c = (crc ^ buf[n]) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

function png(size, pixel) {
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    for (let x = 0; x < size; x++) {
      const [r, g, b, a] = pixel((x + 0.5) / size, (y + 0.5) / size);
      const i = y * (size * 4 + 1) + 1 + x * 4;
      raw[i] = r; raw[i + 1] = g; raw[i + 2] = b; raw[i + 3] = a;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

// Full-bleed indigo, white speech bubble with tail, three indigo "text" bars (vertical, manga style).
function pixel(u, v) {
  const bg = [52, 81, 199, 255];
  const white = [255, 255, 255, 255];
  const dx = (u - 0.5) / 0.34, dy = (v - 0.46) / 0.3;
  const inBubble = dx * dx + dy * dy <= 1;
  const inTail = v > 0.66 && v < 0.84 && u > 0.56 && u < 0.7 && (u - 0.56) / 0.14 > (v - 0.66) / 0.18;
  if (!(inBubble || inTail)) return bg;
  for (const cx of [0.4, 0.5, 0.6]) {
    if (Math.abs(u - cx) < 0.022 && v > 0.32 && v < (cx === 0.4 ? 0.56 : 0.62)) return bg;
  }
  return white;
}

for (const size of [180, 192, 512]) {
  writeFileSync(join(out, `icon-${size}.png`), png(size, pixel));
}
console.log('icons written');
