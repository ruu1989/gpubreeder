'use strict';
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = id => document.getElementById(id);
const GENE_ORDER = ['G', 'Y', 'H', 'W', 'X'];
const PRESETS = ['4G2Y', '3G3Y', '5G1Y', '2G4Y', '6G', '4G1Y1H', '3G2Y1H', 'Best score'];
const STORE = 'gpubreeder-v1';

const state = {
  mix: { G: 3, Y: 3, H: 0, W: 0, X: 0 },
  best: false,
  plans: [],
  sort: 'safe',
  searching: false,
  run: null, // { index, goal, guide, cue, anchor, timerEnd, timerLength }
  crops: [],
};

function genes(text, big) {
  const el = document.createElement('span');
  el.className = 'genes' + (big ? ' big' : '');
  el.setAttribute('aria-label', text);
  for (const letter of text) {
    const i = document.createElement('i');
    i.className = letter;
    i.textContent = letter;
    el.append(i);
  }
  return el;
}

function duration(seconds) {
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.round((total % 3600) / 60);
  if (h) return `${h}h ${String(m).padStart(2, '0')}m`;
  return `${Math.max(1, Math.round(total / 60))} min`;
}

function clock(seconds) {
  const total = Math.max(0, Math.ceil(seconds));
  const m = Math.floor(total / 60);
  return `${m}:${String(total % 60).padStart(2, '0')}`;
}

function growing() {
  return { crop: $('crop').value, fertilized: $('fertilized').checked, quality: Number($('quality').value) / 100 };
}

function mixText() {
  if (state.best) return '';
  return GENE_ORDER.filter(g => state.mix[g]).map(g => `${state.mix[g]}${g}`).join('');
}

function mixTotal() {
  return GENE_ORDER.reduce((sum, g) => sum + state.mix[g], 0);
}

function save() {
  try {
    localStorage.setItem(STORE, JSON.stringify({
      paste: $('paste').value, mix: state.mix, best: state.best, crop: $('crop').value,
      neighbours: $('neighbours').value, seconds: $('seconds').value,
      fertilized: $('fertilized').checked, quality: $('quality').value,
      run: state.run,
    }));
  } catch (_) { /* the app works without saved settings */ }
}

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORE) || 'null');
    if (!saved) return;
    $('paste').value = saved.paste || '';
    Object.assign(state.mix, saved.mix);
    state.best = !!saved.best;
    if (saved.crop) $('crop').value = saved.crop;
    if (saved.neighbours) $('neighbours').value = saved.neighbours;
    if (saved.seconds) $('seconds').value = saved.seconds;
    $('fertilized').checked = saved.fertilized !== false;
    if (saved.quality) $('quality').value = saved.quality;
    state.run = saved.run || null;
  } catch (_) { /* start fresh */ }
}

/* ---------- breed tab ---------- */

function renderMix() {
  const box = $('mix');
  box.replaceChildren();
  for (const gene of GENE_ORDER) {
    const stepper = document.createElement('div');
    stepper.className = 'stepper';
    const letter = genes(gene);
    const value = document.createElement('b');
    value.textContent = state.mix[gene];
    const down = document.createElement('button');
    down.textContent = '−';
    down.setAttribute('aria-label', `Fewer ${gene}`);
    down.onclick = () => bump(gene, -1);
    const up = document.createElement('button');
    up.textContent = '+';
    up.setAttribute('aria-label', `More ${gene}`);
    up.onclick = () => bump(gene, 1);
    stepper.append(letter, value, down, up);
    box.append(stepper);
  }
  const total = mixTotal();
  $('mixState').textContent = state.best ? 'highest score in the time allowed' : total === 6 ? 'in any slots' : `${total} of 6 genes`;
  const presets = $('presets');
  presets.replaceChildren();
  for (const name of PRESETS) {
    const button = document.createElement('button');
    button.textContent = name;
    const current = state.best ? 'Best score' : mixText();
    button.classList.toggle('on', name === current);
    button.onclick = () => applyPreset(name);
    presets.append(button);
  }
  $('go').disabled = !state.best && total !== 6;
  save();
}

function bump(gene, by) {
  state.best = false;
  const next = state.mix[gene] + by;
  if (next < 0 || next > 6 || (by > 0 && mixTotal() >= 6)) return;
  state.mix[gene] = next;
  renderMix();
}

function applyPreset(name) {
  state.best = name === 'Best score';
  if (!state.best) {
    for (const gene of GENE_ORDER) state.mix[gene] = 0;
    for (const [, n, g] of name.matchAll(/(\d)([GYHWX])/g)) state.mix[g] = Number(n);
  }
  renderMix();
}

function placeText(p) {
  return `Box ${p.box}, row ${p.row}, col ${p.column}`;
}

function placeShort(places) {
  const more = places.length > 1 ? ` +${places.length - 1} more` : '';
  return `#${places[0].number}  ${placeText(places[0])}${more}`;
}

let pasteReads = 0;

async function readPaste() {
  const read = ++pasteReads;
  const reply = await invoke('read_plants', { pasted: $('paste').value, ...growing() });
  if (read !== pasteReads) return; // A newer read is on its way, as happens when scanning quickly.
  const list = $('plants');
  list.replaceChildren();
  for (const plant of reply.plants) {
    const row = document.createElement('div');
    row.className = 'chip';
    const note = document.createElement('span');
    note.textContent = plant.places.length ? placeShort(plant.places) : `${plant.to_crossbreed_minutes} min to crossbreed`;
    note.title = `${plant.to_crossbreed_minutes} min to crossbreed`;
    row.append(genes(plant.genes), note);
    list.append(row);
  }
  const last = reply.last;
  $('lastClone').hidden = !last;
  if (last) $('lastClone').textContent = `Last clone is #${last.number}: ${placeText(last)}. If that's not where it really is, a clone was missed or a slot is empty. Add a line with just - at that spot to fix the numbers.`;
  const ignored = $('ignored');
  ignored.hidden = reply.ignored.length === 0;
  ignored.textContent = `Couldn't read: ${reply.ignored.join(', ')}. A plant has six genes.`;
  save();
}

/* ---------- scanning ---------- */

const SCAN_AGREEMENT = 2; // Different frames in a row that must match before a clone counts.
const SCAN_HOLD_MS = 400; // How long to keep watching one spot before searching the whole frame.
const SCAN_PADDING = 12;
const scan = { stream: null, reader: null, canvas: null, context: null, area: null, last: '', streak: 0, taken: '', note: '', lostAt: 0 };

function scanNote(text) {
  if (scan.note === text) return;
  scan.note = text;
  $('scanNote').textContent = text;
  $('scanNote').hidden = !text;
}

function listedPlants() {
  return $('paste').value.split(/\s+/).filter(Boolean);
}

async function startScan() {
  let stream;
  try {
    // Asking for a frame rate alone caps the size, so ask for far more than any screen has.
    stream = await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: { ideal: 60 }, width: { ideal: 7680 }, height: { ideal: 4320 } }, audio: false });
  } catch (_) {
    return; // Pressing Cancel in the picker lands here.
  }
  const track = stream.getVideoTracks()[0];
  const canvas = document.createElement('canvas');
  // This hands over every frame as it arrives, so nothing depends on page timers keeping pace.
  const reader = new MediaStreamTrackProcessor({ track }).readable.getReader();
  Object.assign(scan, { stream, reader, canvas, context: canvas.getContext('2d', { willReadFrequently: true }), area: null, last: '', streak: 0, taken: '', lostAt: 0 });
  track.addEventListener('ended', stopScan);
  readFrames(reader).catch(stopScan);
  $('scanHere').textContent = 'Stop scanning';
  $('scanHere').classList.add('live');
  scanNote('Scanning. Click each clone in Rust so its details panel shows.');
}

async function readFrames(reader) {
  for (;;) {
    const { value: frame, done } = await reader.read();
    if (done) return;
    try {
      scanFrame(frame);
    } finally {
      frame.close();
    }
  }
}

function stopScan() {
  scan.reader?.cancel().catch(() => {});
  scan.stream?.getTracks().forEach(track => track.stop());
  scan.reader = scan.stream = null;
  $('scanHere').textContent = 'Scan clones';
  $('scanHere').classList.remove('live');
  scanNote('');
}

function grabScanArea(frame) {
  const { canvas, context, area } = scan;
  const width = frame.displayWidth, height = frame.displayHeight;
  const left = area ? Math.max(0, area.left - SCAN_PADDING) : 0;
  const up = area ? Math.max(0, area.up - SCAN_PADDING) : 0;
  const right = area ? Math.min(width, area.right + SCAN_PADDING) : width;
  const down = area ? Math.min(height, area.down + SCAN_PADDING) : height;
  canvas.width = Math.round(right - left);
  canvas.height = Math.round(down - up);
  context.drawImage(frame, left, up, canvas.width, canvas.height, 0, 0, canvas.width, canvas.height);
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
  return { pixels, left, up };
}

function scanFrame(frame) {
  const { pixels, left, up } = grabScanArea(frame);
  const got = GeneScan.read(pixels.data, pixels.width, pixels.height);
  if (got.status === 'none' && scan.area) {
    // A brief gap between clones shouldn't cost a slow search of the whole frame.
    scan.lostAt ||= performance.now();
    if (performance.now() - scan.lostAt > SCAN_HOLD_MS) scan.area = null;
    scan.streak = 0;
    return;
  }
  scan.lostAt = 0;
  if (got.area) {
    const { area } = got;
    scan.area = { left: left + area.left, right: left + area.right, up: up + area.up, down: up + area.down };
  }
  if (got.status === 'many') {
    scan.area = null;
    scanNote("Several gene rows are showing. Share the Rust window, not your entire screen.");
  }
  if (got.status === 'unsure') scanNote("Can't read this clone clearly yet. Hold still on it.");
  if (got.status !== 'ok') {
    scan.streak = 0;
    return;
  }
  scan.streak = got.genes === scan.last ? scan.streak + 1 : 1;
  scan.last = got.genes;
  if (scan.streak === SCAN_AGREEMENT && got.genes !== scan.taken) takeClone(got.genes);
}

function takeClone(plant) {
  scan.taken = plant;
  const listed = listedPlants();
  if (listed.includes(plant)) {
    scanNote(`${plant} is already #${listed.indexOf(plant) + 1}.`);
    return;
  }
  const box = $('paste');
  box.value = box.value.trimEnd() + (box.value.trim() ? '\n' : '') + plant;
  scanNote(`Got ${plant}, #${listed.length + 1}. Click the next clone.`);
  chime();
  readPaste();
}

function showError(message) {
  $('error').hidden = !message;
  $('error').textContent = message || '';
}

async function search() {
  if (state.searching) {
    await invoke('stop_search');
    return;
  }
  showError('');
  $('rounds').replaceChildren();
  $('searchNote').textContent = 'Starting the GPU…';
  $('searchTime').textContent = '';
  $('roundsPanel').hidden = false;
  state.plans = [];
  renderPlans();
  setSearching(true);
  try {
    await invoke('start_search', { request: {
      plants: $('paste').value, mix: mixText(), neighbours: Number($('neighbours').value),
      seconds: Number($('seconds').value), ...growing(),
    } });
  } catch (error) {
    setSearching(false);
    showError(String(error));
  }
}

let searchClock = null;

function stopSearchClock() {
  clearInterval(searchClock);
  $('searchClock').hidden = true;
}

// The limit is a ceiling: a search that reaches its target or runs out of routes ends sooner.
function startSearchClock(seconds) {
  stopSearchClock();
  const end = performance.now() + seconds * 1000;
  const show = () => {
    const left = Math.max(0, (end - performance.now()) / 1000);
    const whole = Math.ceil(left);
    $('searchBar').style.width = `${100 * (1 - left / seconds)}%`;
    $('searchLeft').textContent = left > 0
      ? `Up to ${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')} left, less if it finishes sooner`
      : 'Time is up. Finishing the last batch…';
  };
  $('searchClock').hidden = false;
  show();
  searchClock = setInterval(show, 100);
}

function setSearching(on) {
  state.searching = on;
  if (!on) stopSearchClock();
  $('go').textContent = on ? 'Stop and show what was found' : 'Find plans';
  $('go').classList.toggle('stop', on);
}

function sortedPlans() {
  const usable = state.plans.filter(p => p.guide.Ok);
  const key = state.sort === 'fast'
    ? p => [p.crossings, p.guide.Ok.total_seconds, p.guide.Ok.order_steps]
    : p => [p.crossings, p.guide.Ok.order_steps, p.guide.Ok.total_seconds];
  return usable.sort((a, b) => {
    const x = key(a), y = key(b);
    for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return x[i] - y[i];
    return 0;
  });
}

function renderPlans() {
  const box = $('plans');
  box.replaceChildren();
  const plans = sortedPlans();
  $('sortbar').hidden = plans.length < 2;
  if (!plans.length) {
    box.append(emptyPlans());
    return;
  }
  for (const plan of plans) box.append(planCard(plan));
  const failed = state.plans.filter(p => p.guide.Err);
  if (failed.length) {
    const note = document.createElement('p');
    note.className = 'warn';
    note.textContent = `${failed.length} plan(s) hidden: ${failed[0].guide.Err}`;
    box.append(note);
  }
}

function emptyPlans() {
  const empty = document.createElement('div');
  empty.className = 'empty';
  empty.innerHTML = '<h3>Plans appear here</h3><p>Paste your clones, pick the genes you want, and press Find plans.</p>';
  return empty;
}

function planCard(plan) {
  const guide = plan.guide.Ok;
  const card = document.createElement('article');
  card.className = 'plan';
  const head = document.createElement('div');
  head.className = 'plan-head';
  const start = document.createElement('button');
  start.className = 'start';
  start.textContent = 'Start guide';
  start.onclick = () => beginRun(plan);
  head.append(genes(plan.goal), start);

  const facts = document.createElement('div');
  facts.className = 'facts';
  const clones = guide.needs.filter(n => n.starting_plant).reduce((sum, n) => sum + n.count, 0);
  const fact = (value, label) => {
    const d = document.createElement('div');
    const b = document.createElement('b');
    b.textContent = value;
    d.append(b, label);
    return d;
  };
  facts.append(
    fact(plan.crossings, plan.crossings === 1 ? 'crossing' : 'crossings'),
    fact(duration(guide.total_seconds), 'one at a time'),
    fact(duration(guide.parallel_seconds), 'with spare planters'),
    fact(clones, 'starting clones'),
    fact(guide.order_steps, 'order-sensitive'),
  );

  const flow = document.createElement('div');
  flow.className = 'flow';
  plan.steps.forEach((step, i) => {
    const row = document.createElement('div');
    row.className = 'flowstep';
    const label = document.createElement('span');
    label.textContent = `Step ${i + 1}`;
    const parts = document.createElement('div');
    parts.className = 'parts';
    const centreCue = guide.cues.find(c => c.step === i + 1 && c.kind === 'plant_centre');
    parts.append('Centre', genes(centreCue.plants[0]), '+', ...step.order.map(g => genes(g)), '→', genes(step.result));
    if (step.order_required) {
      const pill = document.createElement('span');
      pill.className = 'pill';
      pill.textContent = 'order matters';
      parts.append(pill);
    }
    row.append(label, parts);
    flow.append(row);
  });
  card.append(head, facts, flow);
  return card;
}

/* ---------- guide ---------- */

function beginRun(plan) {
  state.run = { index: plan.index, goal: plan.goal, guide: plan.guide.Ok, cue: 0, anchor: null, timerEnd: null, timerLength: null };
  applyHotkey();
  switchView('guide');
  enterCue();
}

function cue() {
  return state.run.guide.cues[state.run.cue];
}

function enterCue() {
  const run = state.run;
  const c = cue();
  run.timerEnd = null;
  if (c.kind === 'wait' && run.anchor) {
    run.timerEnd = run.anchor + c.wait_until * 1000;
    run.timerLength = c.wait_until * 1000;
    invoke('arm_alarm', { seconds: Math.max(0, (run.timerEnd - Date.now()) / 1000) });
  }
  announce(c);
  save();
  renderGuide();
}

function advance() {
  const run = state.run;
  if (!run) return;
  const c = cue();
  if (c.kind === 'wait' && run.timerEnd > Date.now()) return;
  if (c.kind === 'multiply' && !run.multiplyDone) {
    run.multiplyDone = true;
    run.timerEnd = Date.now() + c.timer_seconds * 1000;
    run.timerLength = c.timer_seconds * 1000;
    invoke('arm_alarm', { seconds: c.timer_seconds });
    speak('Timer started.');
    save();
    renderGuide();
    return;
  }
  if (c.kind === 'multiply' && run.timerEnd > Date.now()) return;
  run.multiplyDone = false;
  if (c.kind === 'plant_centre') run.anchor = Date.now();
  if (c.kind === 'finish') {
    endRun();
    return;
  }
  run.cue += 1;
  enterCue();
}

function endRun() {
  state.run = null;
  invoke('disarm_alarm');
  invoke('set_hotkey', { on: false });
  save();
  renderGuide();
}

let audio = null;

function chime() {
  if (!$('optChime').checked) return;
  audio ??= new AudioContext();
  audio.resume();
  [660, 880, 1100].forEach((pitch, i) => {
    const tone = audio.createOscillator();
    const level = audio.createGain();
    tone.frequency.value = pitch;
    tone.type = 'triangle';
    const at = audio.currentTime + i * 0.16;
    level.gain.setValueAtTime(0.0001, at);
    level.gain.exponentialRampToValueAtTime(0.25, at + 0.02);
    level.gain.exponentialRampToValueAtTime(0.0001, at + 0.3);
    tone.connect(level).connect(audio.destination);
    tone.start(at);
    tone.stop(at + 0.32);
  });
}

function speak(words) {
  if (!$('optVoice').checked) return;
  speechSynthesis.cancel();
  speechSynthesis.speak(new SpeechSynthesisUtterance(words));
}

function spoken(c) {
  const letters = (c.plants[0] || '').split('').join(' ');
  const p = (c.places || [])[0];
  const where = p ? ` Box ${p.box}, row ${p.row}, column ${p.column}.` : '';
  switch (c.kind) {
    case 'plant_centre': return `Plant the centre. ${letters}.${where}`;
    case 'plant_neighbour': return `${c.headline}. ${letters}.${where}`;
    case 'check': return 'Crossbreed done. Check the centre plant.';
    case 'clone': return 'Take the clones.';
    case 'multiply': return c.headline;
    case 'finish': return 'All done. Your plant is ready.';
    default: return c.headline;
  }
}

function announce(c) {
  if (c.kind === 'prepare') return;
  if (c.kind !== 'wait') chime();
  speak(spoken(c));
}

function needRow(need) {
  const row = document.createElement('div');
  row.className = 'need';
  const count = document.createElement('b');
  count.textContent = `×${need.count}`;
  const where = document.createElement('span');
  where.textContent = (need.places || []).length ? placeShort(need.places) : '';
  row.append(genes(need.genes), count, where);
  return row;
}

function placesBox(places) {
  const box = document.createElement('div');
  box.className = 'cueplace';
  box.hidden = places.length === 0;
  places.slice(0, 3).forEach((place, i) => {
    const line = document.createElement('div');
    line.className = i ? 'also' : 'main';
    line.textContent = `${i ? 'or ' : ''}#${place.number}  ${placeText(place)}`;
    box.append(line);
  });
  if (places.length > 3) {
    const more = document.createElement('div');
    more.className = 'also';
    more.textContent = `and ${places.length - 3} more`;
    box.append(more);
  }
  const check = document.createElement('div');
  check.className = 'also';
  check.textContent = 'Check the genes match before you plant it.';
  box.append(check);
  return box;
}

function cueEyebrow(c, steps) {
  return c.step === 0 ? 'Before you start' : c.kind === 'finish' ? 'Finished' : `Crossing ${c.step} of ${steps}`;
}

// One card per cue, built once per guide. Only the current card holds the live controls.
function cueCard(c, index, run) {
  const card = document.createElement('article');
  card.className = 'cue';
  card.dataset.index = index;
  const eyebrow = document.createElement('div');
  eyebrow.className = 'eyebrow';
  eyebrow.textContent = cueEyebrow(c, run.guide.windows.length);
  const head = document.createElement('h1');
  head.textContent = c.headline;
  const big = document.createElement('div');
  big.className = 'bigplants';
  const needs = c.kind === 'prepare' ? run.guide.needs.filter(n => n.starting_plant) : null;
  big.replaceChildren(...(needs ? needs.map(needRow) : c.plants.map(p => genes(p, c.plants.length < 3))));
  const detail = document.createElement('p');
  detail.className = 'detail';
  detail.textContent = c.detail;
  card.append(eyebrow, head, big, placesBox(c.places || []), detail);
  return card;
}

const track = { guide: null, shown: -1 };

function buildTrack(run) {
  const live = $('live');
  live.classList.remove('arrive');
  $('track').after(live); // park the controls before the cards are replaced
  $('track').replaceChildren(...run.guide.cues.map((c, i) => cueCard(c, i, run)));
  track.guide = run.guide;
  track.shown = -1;
}

// Marks each card by its distance from the current cue, and scrolls the current one into place.
function placeTrack(run) {
  if (track.guide !== run.guide) buildTrack(run);
  const cards = $('track').children;
  for (const card of cards) {
    const i = Number(card.dataset.index);
    card.classList.toggle('past', i < run.cue);
    card.classList.toggle('current', i === run.cue);
    card.classList.toggle('next', i === run.cue + 1);
    card.classList.toggle('ahead', i > run.cue + 1);
  }
  const current = cards[run.cue];
  const live = $('live');
  if (live.parentElement !== current) {
    current.append(live);
    live.classList.remove('arrive');
    void live.offsetWidth; // restarts the arrival animation
    live.classList.add('arrive');
  }
  if (track.shown !== run.cue && !$('view-guide').hidden) {
    current.scrollIntoView({ behavior: track.shown < 0 ? 'instant' : 'smooth', block: 'start' });
    track.shown = run.cue;
  }
  return current;
}

function renderGuide() {
  const run = state.run;
  $('guideEmpty').hidden = !!run;
  $('guide').hidden = !run;
  $('navGuide').textContent = run ? `Making ${run.goal}` : 'No plan yet';
  if (!run) return;
  const c = cue();
  const steps = run.guide.windows.length;
  const card = placeTrack(run);
  card.classList.toggle('now', c.kind === 'plant_centre' || c.kind === 'plant_neighbour' || c.kind === 'clone');

  const actions = $('actions');
  actions.replaceChildren();
  const add = (label, handler, primary, key) => {
    const button = document.createElement('button');
    button.className = primary ? 'primary' : 'quiet';
    button.textContent = label;
    if (key && $('optHotkey').checked) {
      const hint = document.createElement('kbd');
      hint.textContent = 'F9';
      button.append(hint);
    }
    button.onclick = handler;
    actions.append(button);
  };
  switch (c.kind) {
    case 'prepare': add("I've got everything", advance, true, true); break;
    case 'plant_centre': add('Centre planted, start timer', advance, true, true); break;
    case 'plant_neighbour': add('Planted', advance, true, true); break;
    case 'check':
      add('Genes match', advance, true, true);
      add("They don't match", () => {
        card.querySelector('.detail').textContent = "Stop here and don't take clones of this plant. Check that no other plant of this crop was nearby and that the planting order was followed, or that the server didn't restart. The crossing can be repeated from this step.";
      });
      break;
    case 'clone': add('Clones taken', advance, true, true); break;
    case 'multiply':
      add(run.multiplyDone ? 'Clones taken' : 'Planted, start timer', advance, true, true);
      actions.firstChild.disabled = run.multiplyDone && run.timerEnd > Date.now();
      break;
    case 'finish': add('End guide', advance, true, true); break;
    default: add('Waiting…', advance, false); actions.firstChild.disabled = true;
  }
  renderPlanter(c);
  const list = $('stepList');
  list.replaceChildren();
  for (let i = 1; i <= steps; i++) {
    const made = run.guide.cues.find(x => x.step === i && x.kind === 'check').plants[0];
    const li = document.createElement('li');
    li.textContent = `${i}. Make ${made}`;
    li.className = i < c.step ? 'done' : i === c.step ? 'current' : '';
    list.append(li);
  }
  $('guideNotes').replaceChildren(...run.guide.notes.map(note => {
    const li = document.createElement('li');
    li.textContent = note;
    return li;
  }));
  tick();
}

function renderPlanter(c) {
  const run = state.run;
  const placed = new Map();
  for (let i = 0; i < run.cue; i++) {
    const past = run.guide.cues[i];
    if (past.step === c.step && past.slot !== null && (past.kind === 'plant_centre' || past.kind === 'plant_neighbour')) placed.set(past.slot, past);
  }
  const box = $('planter');
  box.replaceChildren();
  let order = 0;
  const numbers = new Map();
  for (const x of run.guide.cues) if (x.step === c.step && x.kind === 'plant_neighbour') numbers.set(x.slot, ++order);
  for (let slot = 0; slot < 9; slot++) {
    const cell = document.createElement('div');
    if (slot === 4) { cell.textContent = 'centre'; cell.classList.add('centre'); }
    else if (numbers.has(slot)) cell.textContent = numbers.get(slot);
    if (placed.has(slot)) cell.classList.add('filled');
    if (c.slot === slot && (c.kind === 'plant_centre' || c.kind === 'plant_neighbour')) cell.classList.add('next');
    box.append(cell);
  }
}

function tick() {
  const run = state.run;
  if (!run) return;
  const c = cue();
  const box = $('timerBox');
  const now = Date.now();
  box.hidden = !run.timerEnd;
  if (run.timerEnd) {
    const left = (run.timerEnd - now) / 1000;
    $('clock').textContent = clock(left);
    $('timerBar').style.width = `${Math.min(100, 100 - (left * 100000) / run.timerLength)}%`;
    $('timerLabel').textContent = left > 0 ? 'until the next cue' : 'time is up';
  }
  const late = $('deadline');
  const overdue = c.deadline && run.anchor && now > run.anchor + c.deadline * 1000;
  late.hidden = !overdue;
  if (overdue) late.textContent = 'This is past the safe window, so the plant may miss the crossbreed. Check the centre has not crossbred before you continue.';
}

function alarmFired() {
  if (!state.run) return;
  const c = cue();
  if (c.kind === 'wait') { state.run.timerEnd = Date.now(); advance(); }
  else if (c.kind === 'multiply') { chime(); speak('Take the clones now.'); renderGuide(); }
}

function applyHotkey() {
  invoke('set_hotkey', { on: !!state.run && $('optHotkey').checked }).catch(error => {
    $('optHotkey').checked = false;
    showError(`F9 is in use by another app: ${error}`);
  });
}

/* ---------- grow times ---------- */

async function renderTimes() {
  const { fertilized, quality } = growing();
  const tables = await invoke('grow_table', { fertilized, quality });
  $('timesNote').textContent = `Minutes from planting, at ${fertilized ? 'fertilised' : 'unfertilised'} planter conditions and ${Math.round(quality * 100)}% light, water and heat. Plants move in 60-second ticks, each up to 10% early or late.`;
  const box = $('times');
  box.replaceChildren();
  const seen = new Map();
  for (const table of tables) {
    const key = JSON.stringify(table.rows);
    if (seen.has(key)) { seen.get(key).names.push(table.crop.name); continue; }
    const entry = { names: [table.crop.name], table };
    seen.set(key, entry);
  }
  for (const { names, table } of seen.values()) {
    const panel = document.createElement('article');
    panel.className = 'panel';
    const head = document.createElement('div');
    head.className = 'panel-head';
    const title = document.createElement('h2');
    title.textContent = names.join(', ');
    head.append(title);
    const grid = document.createElement('table');
    const header = grid.insertRow();
    for (const name of ['G genes', 'Clonable', 'Crossbreed', 'After cross', 'Ripe']) {
      const th = document.createElement('th');
      th.textContent = name;
      header.append(th);
    }
    for (const row of table.rows) {
      const tr = grid.insertRow();
      for (const value of [row.g, row.to_sapling, row.to_crossbreed, row.to_mature, row.to_ripe]) tr.insertCell().textContent = value;
    }
    panel.append(head, grid);
    box.append(panel);
  }
}

/* ---------- telemetry ---------- */

const history = [];

function compact(value) {
  if (value >= 1e9) return `${(value / 1e9).toFixed(2)} B`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(0)} M`;
  return value > 0 ? `${(value / 1e3).toFixed(0)} K` : 'idle';
}

function telemetry({ payload: t }) {
  $('gpuName').textContent = t.name;
  const load = t.utilization ?? 0;
  $('util').textContent = t.utilization === null ? '–' : `${load}%`;
  $('ring').style.strokeDashoffset = 327 * (1 - load / 100);
  if (t.groups_per_second > 0 || !t.searching) $('speed').textContent = compact(t.groups_per_second);
  $('temp').textContent = t.temperature === null ? '–' : `${t.temperature} °C`;
  $('power').textContent = t.power_watts === null ? '–' : `${Math.round(t.power_watts)} W`;
  $('core').textContent = t.core_mhz === null ? '–' : `${t.core_mhz} MHz`;
  $('mem').textContent = t.memory_mhz === null ? '–' : `${t.memory_mhz} MHz`;
  $('fan').textContent = t.fan_percent === null ? '–' : `${t.fan_percent}%`;
  if (t.memory_total_gb) {
    $('vram').textContent = `${t.memory_used_gb.toFixed(1)} / ${t.memory_total_gb.toFixed(0)} GB`;
    $('vramBar').style.width = `${(t.memory_used_gb * 100) / t.memory_total_gb}%`;
  }
  history.push(load);
  if (history.length > 120) history.shift();
  const canvas = $('spark');
  const g = canvas.getContext('2d');
  const { width, height } = canvas;
  g.clearRect(0, 0, width, height);
  g.beginPath();
  history.forEach((value, i) => {
    const x = width - (history.length - 1 - i) * (width / 119);
    const y = height - 4 - (value / 100) * (height - 8);
    if (i) g.lineTo(x, y); else g.moveTo(x, y);
  });
  g.strokeStyle = '#e4572e';
  g.lineWidth = 3;
  g.stroke();
}

/* ---------- wiring ---------- */

function switchView(name) {
  for (const view of document.querySelectorAll('.view')) view.hidden = view.id !== `view-${name}`;
  for (const button of document.querySelectorAll('.nav')) button.classList.toggle('active', button.dataset.view === name);
  if (name === 'times') renderTimes();
  if (name === 'guide') {
    track.shown = -1; // a hidden view can't scroll, so place the current cue once it shows
    renderGuide();
  }
}

// Health as the game shows it: the lowest of light, water, ground and temperature. A planter's
// ground is full only with fertiliser, and how full that is depends on the crop.
function renderHealth() {
  const { crop, fertilized, quality } = growing();
  const info = state.crops.find(c => c.key === crop);
  if (!info) return;
  const ground = Math.min(1, (fertilized ? 1 : 0.6) / info.optimal_ground);
  const health = Math.min(ground, quality);
  $('health').textContent = `Health ${Math.round(health * 100)}%, the lowest of ground (${Math.round(ground * 100)}%${fertilized ? '' : ' without fertiliser'}) and light, water and heat (${Math.round(quality * 100)}%). Timers use this, so match it to what the game shows.`;
}

// A running guide takes new timings for the same plan. A wait in progress moves its end time.
function refreshRun() {
  const run = state.run;
  if (!run) return;
  const plan = state.plans.find(p => p.index === run.index && p.goal === run.goal) || state.plans.find(p => p.goal === run.goal);
  const guide = plan && plan.guide.Ok;
  if (!guide || guide.cues.length !== run.guide.cues.length) return;
  run.guide = guide;
  const c = cue();
  if (c.kind === 'wait' && run.anchor && run.timerEnd) {
    run.timerEnd = run.anchor + c.wait_until * 1000;
    run.timerLength = c.wait_until * 1000;
    invoke('arm_alarm', { seconds: Math.max(0, (run.timerEnd - Date.now()) / 1000) });
  }
  save();
  renderGuide();
}

async function startBenchmark() {
  if (state.searching) return;
  try {
    await invoke('benchmark');
  } catch (message) {
    $('benchNote').textContent = String(message);
    return;
  }
  state.searching = true;
  $('bench').disabled = true;
  $('go').disabled = true;
  $('benchNote').textContent = 'Measuring. This takes about 10 seconds…';
}

function endBenchmark(text) {
  state.searching = false;
  $('bench').disabled = false;
  renderMix(); // restores Find plans, which needs a complete target
  $('benchNote').textContent = text;
}

async function replan() {
  save();
  renderHealth();
  await readPaste();
  if (!$('view-times').hidden) renderTimes();
  if (state.plans.length) {
    state.plans = await invoke('replan', growing());
    renderPlans();
    refreshRun();
  }
}

async function main() {
  const crops = await invoke('crops');
  state.crops = crops;
  for (const crop of crops) $('crop').add(new Option(crop.name, crop.key));
  load();
  renderHealth();
  renderMix();
  readPaste();

  for (const button of document.querySelectorAll('.nav')) button.onclick = () => switchView(button.dataset.view);
  for (const button of document.querySelectorAll('.seg')) {
    button.onclick = () => {
      state.sort = button.dataset.sort;
      for (const b of document.querySelectorAll('.seg')) b.classList.toggle('active', b === button);
      renderPlans();
    };
  }
  let debounce;
  $('scanHere').onclick = () => (scan.stream ? stopScan() : startScan());
  $('paste').oninput = () => { clearTimeout(debounce); debounce = setTimeout(readPaste, 120); };
  $('go').onclick = search;
  $('bench').onclick = startBenchmark;
  $('scan').onclick = () => invoke('open_rust_breeder');
  $('credit').onclick = () => invoke('open_rust_breeder');
  for (const id of ['crop', 'fertilized']) $(id).onchange = replan;
  for (const id of ['neighbours', 'seconds']) $(id).onchange = save;
  $('quality').oninput = () => { $('qualityOut').textContent = `${$('quality').value}%`; renderHealth(); };
  $('quality').onchange = replan;
  $('qualityOut').textContent = `${$('quality').value}%`;
  $('testSound').onclick = () => { chime(); speak('Plant this now.'); };
  let quitArmed = null;
  $('quitGuide').onclick = () => {
    if (quitArmed) {
      clearTimeout(quitArmed);
      quitArmed = null;
      $('quitGuide').textContent = 'End guide';
      endRun();
      return;
    }
    $('quitGuide').textContent = 'Press again to end';
    quitArmed = setTimeout(() => {
      quitArmed = null;
      $('quitGuide').textContent = 'End guide';
    }, 3000);
  };
  $('optTop').onchange = () => invoke('set_always_on_top', { on: $('optTop').checked });
  $('optHotkey').onchange = () => { applyHotkey(); renderGuide(); };

  await listen('telemetry', telemetry);
  await listen('alarm', alarmFired);
  await listen('hotkey-next', advance);
  await listen('search-started', ({ payload: seconds }) => startSearchClock(seconds));
  await listen('search-round', ({ payload: r }) => {
    const li = document.createElement('li');
    const left = document.createElement('b');
    left.textContent = `${r.crossings} crossing${r.crossings === 1 ? '' : 's'}`;
    const right = document.createElement('span');
    right.textContent = `${r.new_genotypes.toLocaleString()} new · ${r.seconds.toFixed(2)}s${r.complete ? '' : ' · cut short'}`;
    li.append(left, right);
    $('rounds').append(li);
    $('searchNote').textContent = `${r.known.toLocaleString()} plants reachable so far. Trying ${r.crossings + 1} crossings…`;
  });
  await listen('benchmark-done', ({ payload: b }) => endBenchmark(
    `Top speed ${compact(b.best)} groups/s, median ${compact(b.median)}, over ${b.runs} runs on ${b.device}. ` +
    'Every machine runs the same 100-clone work, so results compare.'));
  await listen('benchmark-failed', ({ payload }) => endBenchmark(`Benchmark failed: ${payload}`));
  await listen('search-failed', ({ payload }) => {
    setSearching(false);
    $('roundsPanel').hidden = true;
    showError(payload);
  });
  await listen('search-done', ({ payload: r }) => {
    setSearching(false);
    if (r.already_have) {
      $('searchNote').textContent = `You already have ${r.already_have}, which meets this target.`;
      return;
    }
    state.plans = r.plans;
    $('searchTime').textContent = `${r.seconds.toFixed(1)}s on ${r.device}`;
    const n = r.proven_up_to;
    const span = `${n} crossing${n === 1 ? '' : 's'}`;
    const best = r.plans.length ? r.plans[0].crossings : 0;
    const proven = best > 0 && best <= n + 1 && !r.stopped;
    if (r.reached_goal) {
      $('searchNote').textContent = proven ? `This is the minimum: every route up to ${span} was searched at this neighbour limit.` : 'A shorter route may exist, because the search ended early.';
    } else {
      const ruled = n ? `No route within ${span} at this neighbour limit. ` : '';
      $('searchNote').textContent = `Target not reached. ${ruled}Try a longer search or more neighbours.`;
    }
    renderPlans();
  });

  if (state.run) {
    applyHotkey();
    switchView('guide');
    const c = cue();
    if (c.kind === 'wait' && state.run.timerEnd) invoke('arm_alarm', { seconds: Math.max(0, (state.run.timerEnd - Date.now()) / 1000) });
    if (c.kind === 'multiply' && state.run.timerEnd) invoke('arm_alarm', { seconds: Math.max(0, (state.run.timerEnd - Date.now()) / 1000) });
  }
  setInterval(tick, 250);
}

main().catch(error => showError(String(error)));
