// Run with: node --test app/tests/scan.test.cjs
// Fixtures are gene rows cropped from real 3440x1440 game frames. Resized copies stand in for other
// displays, though the game would draw those natively.
const test = require('node:test');
const assert = require('node:assert');
const { GeneScan, fixtures, templatesFrom, resized, placed } = require('./helpers.cjs');
global.GeneScan = GeneScan;
require('../ui/scan-templates.js');

const rows = fixtures();
const FIRST_BADGE = { x: 120, y: 46 };
const PITCH = 28;

function edited(row, badge, change) {
  const data = row.data.slice();
  const cx = FIRST_BADGE.x + badge * PITCH, cy = FIRST_BADGE.y;
  for (let y = cy - 10; y <= cy + 10; y++) {
    for (let x = cx - 10; x <= cx + 10; x++) change(data, (y * row.width + x) * 4, x - cx, y - cy);
  }
  return { ...row, data };
}

test('every real clone reads correctly at native size', () => {
  for (const row of rows) {
    assert.strictEqual(GeneScan.read(row.data, row.width, row.height).genes, row.genes);
  }
});

test('clones unseen by the templates are never misread at any display height', () => {
  for (const height of [2160, 1800, 1600, 1440, 1200, 1080, 1024, 900, 800, 768, 720, 600]) {
    for (const row of rows) {
      const others = templatesFrom(rows.filter(o => o.genes !== row.genes));
      const sized = resized(row, height / 1440);
      const got = GeneScan.read(sized.data, sized.width, sized.height, others);
      if (got.status === 'ok') assert.strictEqual(got.genes, row.genes, `${height}p`);
      if (height >= 900) assert.strictEqual(got.status, 'ok', `${row.genes} at ${height}p`);
    }
  }
});

test('position and screen shape make no difference', () => {
  const row = rows[3];
  for (const [width, height] of [[1920, 1080], [2560, 1080], [1280, 1024], [3840, 1600], [1600, 1200]]) {
    for (const [left, top] of [[0, 0], [width - row.width, 0], [0, height - row.height], [width - row.width, height - row.height], [411, 233]]) {
      const frame = placed(row, width, height, left, top);
      assert.strictEqual(GeneScan.read(frame.data, width, height).genes, row.genes, `${width}x${height} at ${left},${top}`);
    }
  }
});

test('the area it reports finds the row again', () => {
  const frame = placed(rows[0], 1920, 1080, 700, 400);
  const { area, genes } = GeneScan.read(frame.data, 1920, 1080);
  assert.strictEqual(genes, rows[0].genes);
  assert.ok(area.left >= 700 && area.right < 1020 && area.up >= 400 && area.down < 492, JSON.stringify(area));
});

test('two rows on show give no read', () => {
  const frame = placed(rows[0], 1200, 700, 50, 50);
  const second = placed(rows[1], 1200, 700, 500, 400);
  for (let y = 0; y < rows[1].height; y++) {
    const from = ((y + 400) * 1200 + 500) * 4;
    frame.data.set(second.data.subarray(from, from + rows[1].width * 4), from);
  }
  assert.strictEqual(GeneScan.read(frame.data, 1200, 700).status, 'many');
});

test('a badge with no letter gives no read', () => {
  for (const row of rows) {
    const blank = edited(row, 2, (data, p, dx, dy) => {
      if (dx * dx + dy * dy <= 49) {
        data[p] = data[(FIRST_BADGE.y - 7) * row.width * 4 + (FIRST_BADGE.x + 2 * PITCH) * 4];
        data[p + 1] = data[(FIRST_BADGE.y - 7) * row.width * 4 + (FIRST_BADGE.x + 2 * PITCH) * 4 + 1];
        data[p + 2] = data[(FIRST_BADGE.y - 7) * row.width * 4 + (FIRST_BADGE.x + 2 * PITCH) * 4 + 2];
      }
    });
    assert.notStrictEqual(GeneScan.read(blank.data, blank.width, blank.height).status, 'ok', row.genes);
  }
});

test('a letter on the wrong colour gives no read', () => {
  for (const row of rows) {
    for (let badge = 0; badge < 6; badge++) {
      const swapped = edited(row, badge, (data, p) => {
        const red = data[p];
        data[p] = data[p + 1];
        data[p + 1] = red;
      });
      const got = GeneScan.read(swapped.data, swapped.width, swapped.height);
      assert.notStrictEqual(got.status, 'ok', `${row.genes} badge ${badge} read as ${got.genes}`);
    }
  }
});

test('a fade or tear between two clones never reads as a third', () => {
  const distinct = rows.filter((row, i) => rows.findIndex(o => o.genes === row.genes) === i);
  const check = (a, b, data) => {
    const got = GeneScan.read(data, a.width, a.height);
    if (got.status === 'ok') assert.ok(got.genes === a.genes || got.genes === b.genes, `${a.genes} to ${b.genes} read as ${got.genes}`);
  };
  for (const a of distinct) {
    for (const b of distinct) {
      if (a === b) continue;
      for (let step = 1; step < 20; step++) {
        const mix = step / 20;
        check(a, b, a.data.map((value, i) => value * (1 - mix) + b.data[i] * mix));
      }
    }
  }
});

test('half a row gives no read', () => {
  const row = rows[0];
  const clipped = { ...row, width: FIRST_BADGE.x + 3 * PITCH - 10 };
  const data = new Uint8Array(clipped.width * row.height * 4);
  for (let y = 0; y < row.height; y++) data.set(row.data.subarray(y * row.width * 4, y * row.width * 4 + clipped.width * 4), y * clipped.width * 4);
  assert.strictEqual(GeneScan.read(data, clipped.width, row.height).status, 'none');
});
