/* Generates the Git Jet source icon (1024x1024 PNG) without any
 * dependencies: renders pixels procedurally and encodes PNG manually.
 * Run: node icon/make-icon.mjs
 */
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 1024;

const pixels = new Uint8Array(SIZE * SIZE * 4);

const BG = [30, 31, 34, 255]; // #1e1f22
const BORDER = [67, 69, 74, 255]; // #43454a
const BLUE = [53, 116, 240, 255]; // #3574f0
const TRANSPARENT = [0, 0, 0, 0];

const RADIUS = 220;

function setBackground() {
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      pixels[idx(x, y)] = inRoundRect(x, y) ? BG : TRANSPARENT;
    }
  }
  // Subtle inner border.
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      if (!inRoundRect(x, y)) continue;
      if (!shrunkRoundRect(x, y, 10) && inRoundRect(x, y)) pixels[idx(x, y)] = BORDER;
    }
  }
}

function idx(x, y) {
  return (y * SIZE + x) * 4;
}

function inRoundRect(x, y, pad = 0) {
  const minX = pad, minY = pad, maxX = SIZE - 1 - pad, maxY = SIZE - 1 - pad;
  const r = RADIUS;
  if (x < minX || x > maxX || y < minY || y > maxY) return false;
  const cx = Math.min(Math.max(x, minX + r), maxX - r);
  const cy = Math.min(Math.max(y, minY + r), maxY - r);
  const dx = x - cx, dy = y - cy;
  return dx * dx + dy * dy <= r * r;
}
function shrunkRoundRect(x, y, pad) {
  return inRoundRect(x, y, pad);
}

function blend(over, x, y, alpha) {
  if (alpha <= 0) return;
  const i = idx(x, y);
  const a = Math.min(1, alpha);
  const base = pixels[i];
  if (a >= 1) {
    for (let c = 0; c < 4; c++) pixels[i + c] = over[c];
    return;
  }
  const blended = base.map((value, index) => Math.round(value * (1 - a) + over[index] * a));
  for (let c = 0; c < 4; c++) pixels[i + c] = blended[c];
}

function fillDonut(cx, cy, outerR, innerR, color) {
  for (let y = cy - outerR - 2; y <= cy + outerR + 2; y++) {
    for (let x = cx - outerR - 2; x <= cx + outerR + 2; x++) {
      const d = Math.hypot(x - cx, y - cy);
      if (d <= innerR) continue;
      if (d <= outerR) blend(color, x, y, Math.min(1, outerR - d + 1));
    }
  }
}

function strokePolyline(points, width, color) {
  const radius = width / 2;
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      let minDist = Infinity;
      for (let p = 0; p < points.length - 1; p++) {
        const dist = distToSegment(x, y, points[p], points[p + 1]);
        if (dist < minDist) minDist = dist;
      }
      if (minDist < radius) blend(color, x, y, Math.min(1, radius - minDist + 1));
    }
  }
}

function distToSegment(px, py, a, b) {
  const dx = b[0] - a[0], dy = b[1] - a[1];
  const lengthSq = dx * dx + dy * dy;
  let t = lengthSq === 0 ? 0 : ((px - a[0]) * dx + (py - a[1]) * dy) / lengthSq;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(px - (a[0] + t * dx), py - (a[1] + t * dy));
}

function quadratic(p0, p1, p2, steps = 60) {
  const points = [];
  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    const mt = 1 - t;
    points.push([
      mt * mt * p0[0] + 2 * mt * t * p1[0] + t * t * p2[0],
      mt * mt * p0[1] + 2 * mt * t * p1[1] + t * t * p2[1],
    ]);
  }
  return points;
}

setBackground();

const LINE_W = 34;
// Left branch: down, then curve into the bottom node.
const left = [
  ...rectPoints(380, 360, 380, 540),
];
const leftCurve = quadratic([380, 540], [380, 690], [512, 742]);
strokePolyline(left, LINE_W, BLUE);
strokePolyline(leftCurve, LINE_W, BLUE);

// Right branch.
const right = rectPoints(644, 360, 644, 540);
const rightCurve = quadratic([644, 540], [644, 690], [512, 742]);
strokePolyline(right, LINE_W, BLUE);
strokePolyline(rightCurve, LINE_W, BLUE);

// Nodes (donuts).
fillDonut(380, 300, 62, 30, BLUE);
fillDonut(644, 300, 62, 30, BLUE);
fillDonut(512, 786, 62, 30, BLUE);

function rectPoints(x1, y1, x2, y2) {
  // Expand into two points; strokePolyline expects segment lists.
  return [[x1, y1], [x2, y2]];
}

/* ---------------------------------------------------------------- PNG encode */

const rawData = Buffer.alloc((SIZE * 4 + 1) * SIZE);
let cursor = 0;
for (let y = 0; y < SIZE; y++) {
  rawData[cursor++] = 0; // filter: none
  for (let x = 0; x < SIZE; x++) {
    const i = (y * SIZE + x) * 4;
    rawData[cursor++] = pixels[i];
    rawData[cursor++] = pixels[i + 1];
    rawData[cursor++] = pixels[i + 2];
    rawData[cursor++] = 255; // fully opaque inside the round rect, see bg
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type: RGBA

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(rawData, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crcInput = Buffer.concat([typeBuf, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(crcInput), 0);
  return Buffer.concat([length, typeBuf, data, crc]);
}

function crc32(buffer) {
  crc32.table ||= (() => {
    const table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      table[n] = c >>> 0;
    }
    return table;
  })();
  let crc = 0xffffffff;
  for (const byte of buffer) crc = crc32.table[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

const target = "src-tauri/icons/icon-source.png";
mkdirSync(dirname(target), { recursive: true });
writeFileSync(target, png);
console.log("icon written to " + target + " (" + png.length + " bytes)");
