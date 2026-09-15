// minimal PNG writer — draws a bar-chart icon
const zlib = require("node:zlib");
const fs = require("node:fs");

const W = 256, H = 256;
const px = Buffer.alloc(W * H * 4);

function crc32(buf) {
  let c, table = crc32.t;
  if (!table) {
    table = crc32.t = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      table[n] = c >>> 0;
    }
  }
  c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = table[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const t = Buffer.from(type, "ascii");
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
}

const R = 56; // corner radius
function inRounded(x, y) {
  const cx = x < R ? R : x >= W - R ? W - R - 1 : x;
  const cy = y < R ? R : y >= H - R ? H - R - 1 : y;
  return (x - cx) ** 2 + (y - cy) ** 2 <= R * R;
}

// bars: 4 ascending bars
const bars = [
  { x: 46,  w: 30, h: 70,  c: [31, 111, 235] },
  { x: 92,  w: 30, h: 110, c: [47, 129, 247] },
  { x: 138, w: 30, h: 150, c: [88, 166, 255] },
  { x: 184, w: 30, h: 190, c: [163, 203, 255] },
];

for (let y = 0; y < H; y++) {
  for (let x = 0; x < W; x++) {
    const i = (y * W + x) * 4;
    if (!inRounded(x, y)) { px[i + 3] = 0; continue; }
    // background: subtle vertical gradient #161b22 → #1c2230
    const t = y / H;
    px[i] = Math.round(22 + 6 * t);
    px[i + 1] = Math.round(27 + 7 * t);
    px[i + 2] = Math.round(34 + 14 * t);
    px[i + 3] = 255;
    for (const b of bars) {
      const top = H - 34 - b.h;
      if (x >= b.x && x < b.x + b.w && y >= top && y < H - 34) {
        const br = 6; // bar corner radius
        const bx = Math.min(Math.max(x, b.x + br), b.x + b.w - br - 1);
        const by = Math.min(Math.max(y, top + br), H - 34 - 1);
        if ((x - bx) ** 2 + (y - by) ** 2 <= br * br || y >= top + br) {
          px[i] = b.c[0]; px[i + 1] = b.c[1]; px[i + 2] = b.c[2];
        }
      }
    }
  }
}

// scanlines with filter byte 0
const raw = Buffer.alloc(H * (W * 4 + 1));
for (let y = 0; y < H; y++) {
  raw[y * (W * 4 + 1)] = 0;
  px.copy(raw, y * (W * 4 + 1) + 1, y * W * 4, (y + 1) * W * 4);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0); ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA

const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk("IHDR", ihdr),
  chunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
fs.writeFileSync(__dirname + "/icon.png", png);
console.log("wrote icon.png", png.length, "bytes");
