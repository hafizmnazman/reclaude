// Generates the Reclaude app icon: an original 10-ray terracotta spark on a
// dark rounded square, written as a 1024x1024 PNG with zero npm dependencies.
// Run: node scripts/gen-icon.mjs   then: npx tauri icon scripts/icon-1024.png
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 1024;
const SS = 3; // supersampling factor per axis

// palette
const BG = [0x26, 0x26, 0x24]; // Claude dark background
const SPARK_EDGE = [0xd9, 0x77, 0x57]; // terracotta
const SPARK_CENTER = [0xe5, 0x8c, 0x66]; // slightly lighter center

const CX = SIZE / 2;
const CY = SIZE / 2;
const MARGIN = 76;
const CORNER = 210;
const RAYS = 10;
const ROT = -Math.PI / 2;
const R_MIN = 178;
const R_MAX = 396;

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

function sparkRadius(theta) {
  const t = 0.5 + 0.5 * Math.cos(RAYS * (theta - ROT));
  return R_MIN + (R_MAX - R_MIN) * Math.pow(t, 1.6); // soft rounded rays
}

function roundedRectAlpha(x, y) {
  const half = SIZE / 2 - MARGIN - CORNER;
  const dx = Math.max(Math.abs(x - CX) - half, 0);
  const dy = Math.max(Math.abs(y - CY) - half, 0);
  const d = Math.hypot(dx, dy) - CORNER;
  return clamp(0.5 - d / 1.5, 0, 1);
}

function sparkSample(x, y) {
  const dx = x - CX;
  const dy = y - CY;
  const r = Math.hypot(dx, dy);
  const rr = sparkRadius(Math.atan2(dy, dx));
  const a = clamp((rr - r) / 1.5 + 0.5, 0, 1);
  const t = clamp(r / rr, 0, 1);
  return [a, t];
}

const rgba = Buffer.alloc(SIZE * SIZE * 4);
for (let py = 0; py < SIZE; py++) {
  for (let px = 0; px < SIZE; px++) {
    let bgA = 0;
    let spA = 0;
    let spT = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const x = px + (sx + 0.5) / SS;
        const y = py + (sy + 0.5) / SS;
        bgA += roundedRectAlpha(x, y);
        const [a, t] = sparkSample(x, y);
        spA += a;
        spT += a * t;
      }
    }
    const n = SS * SS;
    bgA /= n;
    const tAvg = spA > 0 ? spT / spA : 0;
    spA /= n;

    const spark = [
      SPARK_CENTER[0] + (SPARK_EDGE[0] - SPARK_CENTER[0]) * tAvg,
      SPARK_CENTER[1] + (SPARK_EDGE[1] - SPARK_CENTER[1]) * tAvg,
      SPARK_CENTER[2] + (SPARK_EDGE[2] - SPARK_CENTER[2]) * tAvg,
    ];

    // composite: spark over background plate over transparency
    const sparkOnPlate = spA * bgA; // keep the spark clipped to the plate
    const outA = sparkOnPlate + bgA * (1 - sparkOnPlate);
    const o = (py * SIZE + px) * 4;
    if (outA > 0) {
      for (let c = 0; c < 3; c++) {
        const col =
          (spark[c] * sparkOnPlate + BG[c] * bgA * (1 - sparkOnPlate)) / outA;
        rgba[o + c] = Math.round(clamp(col, 0, 255));
      }
      rgba[o + 3] = Math.round(outA * 255);
    }
  }
}

// ---- minimal PNG writer ----
const crcTable = new Int32Array(256);
for (let i = 0; i < 256; i++) {
  let c = i;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  crcTable[i] = c;
}
function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return ~c >>> 0;
}
function chunk(type, data) {
  const out = Buffer.alloc(8 + data.length + 4);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "ascii");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const raw = Buffer.alloc((SIZE * 4 + 1) * SIZE);
for (let y = 0; y < SIZE; y++) {
  rgba.copy(raw, y * (SIZE * 4 + 1) + 1, y * SIZE * 4, (y + 1) * SIZE * 4);
}
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

const here = dirname(fileURLToPath(import.meta.url));
mkdirSync(here, { recursive: true });
const out = join(here, "icon-1024.png");
writeFileSync(out, png);
console.log("wrote", out);
