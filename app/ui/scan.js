// Reads a clone's genes from a captured frame. Nothing here depends on where the panel sits or on
// the screen's shape: it looks for six round gene badges evenly spaced in a row, at any size.
// Green badges can only be G, Y or H and red ones only W or X, so colour checks every letter.
// Anything unclear gives no read, because a missed clone is cheap and a wrong one is not.

const GeneScan = (() => {
  const GREEN = 1, RED = 2, LIGHT = 3;
  const MIN_BADGE = 8, MAX_BADGE = 70;
  const GRID = 16;
  const MIN_MATCH = 0.72;
  const MIN_LEAD = 0.08;
  const LETTERS_BY_COLOUR = { [GREEN]: ['G', 'H', 'Y'], [RED]: ['W', 'X'] };

  function maskOf(data, width, height) {
    const mask = new Uint8Array(width * height);
    for (let i = 0, p = 0; i < mask.length; i++, p += 4) {
      const r = data[p], g = data[p + 1], b = data[p + 2];
      if (g > r + 16 && g > b + 16) mask[i] = GREEN;
      else if (r > g + 28 && r > b + 28) mask[i] = RED;
      else if (r > 150 && g > 150 && b > 150) mask[i] = LIGHT;
    }
    return mask;
  }

  // `reach` 2 joins pixels one apart, because blending around a letter can split a badge. At small
  // sizes that also swallows the dashes between badges, so rowsIn falls back to reach 1.
  function blobsOf(mask, width, height, reach) {
    const seen = new Uint8Array(mask.length);
    const stack = new Int32Array(mask.length);
    const blobs = [];
    for (let start = 0; start < mask.length; start++) {
      if (!mask[start] || seen[start]) continue;
      let top = 0, area = 0;
      const counts = [0, 0, 0, 0];
      let left = width, right = 0, up = height, down = 0;
      stack[top++] = start;
      seen[start] = 1;
      while (top) {
        const at = stack[--top];
        const x = at % width, y = (at - x) / width;
        area++;
        counts[mask[at]]++;
        if (x < left) left = x;
        if (x > right) right = x;
        if (y < up) up = y;
        if (y > down) down = y;
        for (let dy = -reach; dy <= reach; dy++) {
          const ny = y + dy;
          if (ny < 0 || ny >= height) continue;
          for (let dx = -reach; dx <= reach; dx++) {
            const nx = x + dx;
            if (nx < 0 || nx >= width) continue;
            const next = ny * width + nx;
            if (mask[next] && !seen[next]) {
              seen[next] = 1;
              stack[top++] = next;
            }
          }
        }
      }
      blobs.push({ left, right, up, down, area, green: counts[GREEN], red: counts[RED], light: counts[LIGHT] });
    }
    return blobs;
  }

  function badgeFrom(blob) {
    const w = blob.right - blob.left + 1, h = blob.down - blob.up + 1;
    if (w < MIN_BADGE || h < MIN_BADGE || w > MAX_BADGE || h > MAX_BADGE) return null;
    if (Math.abs(w - h) > Math.max(2, 0.2 * w)) return null;
    if (blob.area < 0.5 * w * h) return null;
    const coloured = blob.green + blob.red;
    if (coloured < 0.3 * blob.area || blob.light < 0.05 * blob.area) return null;
    if (Math.max(blob.green, blob.red) < 0.8 * coloured) return null;
    return { ...blob, size: (w + h) / 2, x: (blob.left + blob.right) / 2, y: (blob.up + blob.down) / 2, colour: blob.green > blob.red ? GREEN : RED };
  }

  // Every run of six same-sized badges on one line with even gaps.
  function rowsOf(badges) {
    const rows = [];
    const used = new Set();
    const sorted = [...badges].sort((a, b) => a.y - b.y || a.x - b.x);
    for (const first of sorted) {
      if (used.has(first)) continue;
      const line = sorted
        .filter(b => Math.abs(b.y - first.y) <= 0.2 * first.size && Math.abs(b.size - first.size) <= 0.25 * first.size)
        .sort((a, b) => a.x - b.x);
      for (let i = 0; i + 6 <= line.length; i++) {
        const six = line.slice(i, i + 6);
        const pitch = (six[5].x - six[0].x) / 5;
        const even = six.every((b, k) => Math.abs(b.x - (six[0].x + k * pitch)) <= 0.12 * pitch);
        if (even && pitch >= 1.2 * first.size && pitch <= 2.2 * first.size) {
          six.forEach(b => used.add(b));
          rows.push(six);
          i += 5;
        }
      }
    }
    return rows;
  }

  // A square of how white each part of the badge's middle is, 0 to 1.
  function letterShape(data, width, badge) {
    const span = 0.74 * badge.size;
    const near = (x, y) => {
      const p = (Math.round(y) * width + Math.round(x)) * 4;
      return Math.min(data[p], data[p + 1], data[p + 2]);
    };
    const shape = new Float32Array(GRID * GRID);
    const steps = 3;
    let low = 255, high = 0;
    for (let gy = 0; gy < GRID; gy++) {
      for (let gx = 0; gx < GRID; gx++) {
        let sum = 0;
        for (let sy = 0; sy < steps; sy++) {
          for (let sx = 0; sx < steps; sx++) {
            const x = badge.x + span * ((gx + (sx + 0.5) / steps) / GRID - 0.5);
            const y = badge.y + span * ((gy + (sy + 0.5) / steps) / GRID - 0.5);
            sum += near(x, y);
          }
        }
        const value = sum / (steps * steps);
        shape[gy * GRID + gx] = value;
        if (value < low) low = value;
        if (value > high) high = value;
      }
    }
    if (high - low < 30) return null;
    for (let i = 0; i < shape.length; i++) shape[i] = (shape[i] - low) / (high - low);
    return shape;
  }

  function correlation(a, b) {
    let meanA = 0, meanB = 0;
    for (let i = 0; i < a.length; i++) { meanA += a[i]; meanB += b[i]; }
    meanA /= a.length; meanB /= b.length;
    let top = 0, sumA = 0, sumB = 0;
    for (let i = 0; i < a.length; i++) {
      const x = a[i] - meanA, y = b[i] - meanB;
      top += x * y; sumA += x * x; sumB += y * y;
    }
    return top / Math.sqrt(sumA * sumB || 1);
  }

  function letterOf(shape, colour, templates) {
    const ranked = LETTERS_BY_COLOUR[colour]
      .filter(letter => templates[letter])
      .map(letter => ({ letter, score: correlation(shape, templates[letter]) }))
      .sort((a, b) => b.score - a.score);
    const [best, next] = ranked;
    if (!best) return null;
    const lead = best.score - (next ? next.score : -1);
    return { letter: best.letter, score: best.score, lead, sure: best.score >= MIN_MATCH && lead >= MIN_LEAD };
  }

  function rowsIn(data, width, height) {
    const mask = maskOf(data, width, height);
    for (const reach of [2, 1]) {
      const rows = rowsOf(blobsOf(mask, width, height, reach).map(badgeFrom).filter(Boolean));
      if (rows.length) return rows;
    }
    return [];
  }

  /** status: 'ok' with genes, 'none', 'many' when several rows are on show, or 'unsure'. */
  function read(data, width, height, templates = TEMPLATES) {
    const rows = rowsIn(data, width, height);
    if (rows.length === 0) return { status: 'none' };
    if (rows.length > 1) return { status: 'many', rows: rows.length };
    const row = rows[0];
    const letters = row.map(badge => {
      const shape = letterShape(data, width, badge);
      return shape && letterOf(shape, badge.colour, templates);
    });
    const size = row[0].size, pitch = (row[5].x - row[0].x) / 5;
    const area = { left: row[0].left - pitch, right: row[5].right + pitch, up: row[0].up - size, down: row[0].down + size };
    if (letters.some(l => !l || !l.sure)) return { status: 'unsure', area, letters };
    return { status: 'ok', genes: letters.map(l => l.letter).join(''), area, letters };
  }

  const TEMPLATES = {};

  const shapes = (data, width, height) => rowsIn(data, width, height).map(row => row.map(badge => ({ shape: letterShape(data, width, badge), colour: badge.colour })));

  return { read, shapes, TEMPLATES };
})();

if (typeof module !== 'undefined') module.exports = GeneScan;
