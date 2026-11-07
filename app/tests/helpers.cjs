const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const GeneScan = require('../ui/scan.js');

function fixtures() {
  const list = JSON.parse(fs.readFileSync(path.join(__dirname, 'fixtures', 'gene-rows.json')));
  return list.map(f => ({ ...f, data: new Uint8Array(zlib.gunzipSync(Buffer.from(f.rgba, 'base64'))) }));
}

/** Averages each letter's shape across the given rows. */
function templatesFrom(rows) {
  const sums = {};
  for (const row of rows) {
    const [found] = GeneScan.shapes(row.data, row.width, row.height);
    found.forEach((badge, i) => {
      const letter = row.genes[i];
      sums[letter] ??= { total: new Float32Array(badge.shape.length), count: 0 };
      sums[letter].total.forEach((_, k) => { sums[letter].total[k] += badge.shape[k]; });
      sums[letter].count++;
    });
  }
  return Object.fromEntries(Object.entries(sums).map(([letter, s]) => [letter, s.total.map(v => v / s.count)]));
}

/** Bilinear resize, averaging first when shrinking a lot. */
function resized(row, scale) {
  const width = Math.round(row.width * scale), height = Math.round(row.height * scale);
  const data = new Uint8Array(width * height * 4);
  const steps = scale < 1 ? Math.ceil(1 / scale) + 1 : 1;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const sum = [0, 0, 0, 0];
      for (let sy = 0; sy < steps; sy++) {
        for (let sx = 0; sx < steps; sx++) {
          const fx = Math.min(row.width - 1.001, (x + (sx + 0.5) / steps) / scale);
          const fy = Math.min(row.height - 1.001, (y + (sy + 0.5) / steps) / scale);
          const x0 = Math.floor(fx), y0 = Math.floor(fy), ax = fx - x0, ay = fy - y0;
          for (let c = 0; c < 4; c++) {
            const at = (xx, yy) => row.data[(yy * row.width + xx) * 4 + c];
            sum[c] += (at(x0, y0) * (1 - ax) + at(x0 + 1, y0) * ax) * (1 - ay) + (at(x0, y0 + 1) * (1 - ax) + at(x0 + 1, y0 + 1) * ax) * ay;
          }
        }
      }
      for (let c = 0; c < 4; c++) data[(y * width + x) * 4 + c] = sum[c] / (steps * steps);
    }
  }
  return { ...row, width, height, data };
}

/** Puts a row somewhere on a dim speckled frame, as the game's busy backdrop would. */
function placed(row, width, height, left, top, seed = 1) {
  const data = new Uint8Array(width * height * 4);
  let r = seed * 2654435761 >>> 0;
  for (let i = 0; i < data.length; i += 4) {
    r = (r * 1664525 + 1013904223) >>> 0;
    const v = 30 + (r >>> 26);
    data[i] = v; data[i + 1] = v; data[i + 2] = v; data[i + 3] = 255;
  }
  for (let y = 0; y < row.height; y++) {
    const from = y * row.width * 4;
    data.set(row.data.subarray(from, from + row.width * 4), ((y + top) * width + left) * 4);
  }
  return { ...row, width, height, data };
}

module.exports = { GeneScan, fixtures, templatesFrom, resized, placed };
