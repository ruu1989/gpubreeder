'use strict';
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = id => document.getElementById(id);
const GENE_ORDER = ['G', 'Y', 'H', 'W', 'X'];
const PRESETS = ['4G2Y', '3G3Y', '5G1Y', '2G4Y', '6G', '4G1Y1H', '3G2Y1H', 'Best score'];
const STORE = 'gpubreeder-v2';
const OLD_STORE = 'gpubreeder-v1';

const state = {
  tabs: [], // one per crop being bred, see newTab
  active: null,
  sort: 'safe',
  searching: false,
  searchTab: null, // the tab whose search is running, or null for a benchmark
  fullWater: true,
  crops: [],
};

// The Breed and Guide pages always show the active crop tab. These read and write its fields,
// so code written for one crop works on whichever tab is showing.
for (const key of ['mix', 'best', 'plans', 'run']) {
  Object.defineProperty(state, key, { get: () => tab()[key], set: value => { tab()[key] = value; } });
}

function tab() {
  return state.tabs.find(t => t.id === state.active) || state.tabs[0];
}

function tabById(id) {
  return state.tabs.find(t => t.id === id);
}

function newTab(crop, from) {
  return {
    id: `t${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    crop,
    paste: '',
    mix: { G: 3, Y: 3, H: 0, W: 0, X: 0 },
    best: false,
    neighbours: from ? from.neighbours : '5',
    seconds: from ? from.seconds : '60',
    plans: [],
    search: { rounds: [], note: '', time: '', shown: false },
    run: null, // { index, goal, guide, cue, anchor, anchorStep, timerEnd, timerLength }
    attention: false,
  };
}

function cropName(key) {
  return (state.crops.find(c => c.key === key) || { name: key }).name;
}

// Tabs are named by crop, numbered when a crop has more than one.
function tabName(t) {
  const same = state.tabs.filter(x => x.crop === t.crop);
  return same.length > 1 ? `${cropName(t.crop)} ${same.indexOf(t) + 1}` : cropName(t.crop);
}

function runningTabs() {
  return state.tabs.filter(t => t.run);
}

function genes(text, big) {
  const el = document.createElement('span');
  el.className = 'genes' + (big === 'mid' ? ' mid' : big ? ' big' : '');
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

// Light and heat are taken as perfect. Fertiliser and water are the two choices that vary.
function growing(t = tab()) {
  return { crop: t.crop, fertilized: $('fertilized').checked, fullWater: state.fullWater };
}

function mixText() {
  if (state.best) return '';
  return GENE_ORDER.filter(g => state.mix[g]).map(g => `${state.mix[g]}${g}`).join('');
}

function mixTotal() {
  return GENE_ORDER.reduce((sum, g) => sum + state.mix[g], 0);
}

// The page's inputs belong to the active tab, so they're copied back into it before saving.
function save() {
  const t = tab();
  if (t) Object.assign(t, { paste: $('paste').value, neighbours: $('neighbours').value, seconds: $('seconds').value });
  try {
    localStorage.setItem(STORE, JSON.stringify({
      tabs: state.tabs.map(x => ({
        id: x.id, crop: x.crop, paste: x.paste, mix: x.mix, best: x.best,
        neighbours: x.neighbours, seconds: x.seconds, run: x.run,
      })),
      active: state.active, fertilized: $('fertilized').checked, fullWater: state.fullWater,
    }));
  } catch (_) { /* the app works without saved settings */ }
}

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORE) || 'null');
    if (saved) {
      state.tabs = saved.tabs.map(x => Object.assign(newTab(x.crop), x, { plans: [], attention: false }));
      state.active = saved.active;
      $('fertilized').checked = saved.fertilized !== false;
      state.fullWater = saved.fullWater !== false;
    } else {
      // Settings from before crop tabs become the first tab.
      const old = JSON.parse(localStorage.getItem(OLD_STORE) || 'null');
      if (old) {
        const t = newTab(old.crop || state.crops[0].key);
        Object.assign(t, { paste: old.paste || '', best: !!old.best, run: old.run || null });
        Object.assign(t.mix, old.mix);
        if (old.neighbours) t.neighbours = old.neighbours;
        if (old.seconds) t.seconds = old.seconds;
        state.tabs = [t];
        $('fertilized').checked = old.fertilized !== false;
      }
    }
  } catch (_) { /* start fresh */ }
  if (!state.tabs.length) state.tabs = [newTab(state.crops[0].key)];
  if (!tabById(state.active)) state.active = state.tabs[0].id;
  showTabInputs();
}

function showTabInputs() {
  const t = tab();
  $('paste').value = t.paste;
  $('crop').value = t.crop;
  $('neighbours').value = t.neighbours;
  $('seconds').value = t.seconds;
}

/* ---------- crop tabs ---------- */

function tabStatus(t) {
  if (state.searching && state.searchTab === t.id) return 'Searching…';
  if (t.closing) return state.tabs.length > 1 ? 'Press × again to remove' : 'Press × again to clear';
  const run = t.run;
  if (run) {
    const step = `Step ${run.cue + 1} of ${run.guide.cues.length}`;
    if (run.timerEnd) {
      const left = (run.timerEnd - Date.now()) / 1000;
      return left > 0 ? `${step} · about ${clock(left)}` : `${step} · check the plant`;
    }
    return `${step} · ${run.guide.cues[run.cue].headline}`;
  }
  const plans = t.plans.filter(p => p.guide.Ok).length;
  if (plans) return `${plans} plan${plans === 1 ? '' : 's'} found`;
  const clones = t.paste.split(/\s+/).filter(x => x && x !== '-').length;
  return clones ? `${clones} clone${clones === 1 ? '' : 's'}` : 'No clones yet';
}

// A running guide whose estimate has passed needs a look, as does one the alarm flagged.
function needsLook(t) {
  return t.attention || !!(t.run && t.run.timerEnd && t.run.timerEnd <= Date.now());
}

function renderTabs() {
  const strip = $('cropTabs');
  const add = $('addCrop');
  strip.replaceChildren();
  for (const t of state.tabs) {
    const button = document.createElement('button');
    button.className = 'croptab';
    button.dataset.id = t.id;
    button.setAttribute('role', 'tab');
    button.setAttribute('aria-selected', String(t.id === state.active));
    button.classList.toggle('active', t.id === state.active);
    const name = document.createElement('b');
    name.textContent = tabName(t);
    const status = document.createElement('span');
    status.className = 'status';
    button.append(name, status);
    const close = document.createElement('span');
    close.className = 'close';
    close.textContent = '×';
    close.title = state.tabs.length > 1 ? `Remove ${tabName(t)}` : `Clear ${tabName(t)} and start fresh`;
    close.onclick = event => { event.stopPropagation(); removeTab(t.id); };
    button.append(close);
    button.onclick = () => switchTab(t.id);
    strip.append(button);
  }
  strip.append(add);
  updateTabs();
  const running = runningTabs();
  $('navGuide').textContent = running.length > 1 ? `${running.length} guides running` : running.length ? `Making ${running[0].run.goal}` : 'No plan yet';
}

// Refreshes each tab's status line without rebuilding the strip.
function updateTabs() {
  for (const button of $('cropTabs').querySelectorAll('.croptab')) {
    const t = tabById(button.dataset.id);
    if (!t) continue;
    button.querySelector('.status').textContent = tabStatus(t);
    button.classList.toggle('alert', t.id !== state.active && needsLook(t));
    button.classList.toggle('running', !!t.run);
  }
}

function switchTab(id) {
  if (id === state.active || !tabById(id)) return;
  save();
  state.active = id;
  tab().attention = false;
  showTabInputs();
  showError('');
  renderMix();
  renderHealth();
  readPaste();
  renderSearchPanel();
  renderPlans();
  track.shown = -1;
  renderGuide();
  renderTabs();
  save();
}

function addTab(crop) {
  const t = newTab(crop, tab());
  state.tabs.push(t);
  switchTab(t.id);
}

// Removing a tab with clones, plans or a guide takes a second press. The last tab is cleared
// instead, so there's always one to work in.
function removeTab(id) {
  const t = tabById(id);
  if (!t || (state.searching && state.searchTab === id)) return;
  const busy = t.run || t.plans.length || t.paste.trim();
  if (busy && !t.closing) {
    t.closing = setTimeout(() => { t.closing = null; updateTabs(); }, 3000);
    updateTabs();
    return;
  }
  clearTimeout(t.closing);
  t.closing = null;
  invoke('forget_tab', { tab: id });
  if (state.tabs.length < 2) {
    clearTab(t);
    return;
  }
  const at = state.tabs.indexOf(t);
  state.tabs.splice(at, 1);
  if (state.active === id) {
    state.active = null;
    switchTab(state.tabs[Math.max(0, at - 1)].id);
  }
  applyHotkey();
  renderTabs();
  save();
}

// Empties a tab's clones, plans, search and guide. Its crop, target and settings stay.
function clearTab(t) {
  Object.assign(t, { paste: '', plans: [], run: null, attention: false, search: { rounds: [], note: '', time: '', shown: false } });
  invoke('disarm_alarm', { tab: t.id });
  if (t.id === state.active) {
    scan.taken = ''; // so a clone scanned just before can be taken again
    showTabInputs();
    showError('');
    readPaste();
    renderSearchPanel();
    renderPlans();
    renderGuide();
  }
  applyHotkey();
  renderTabs();
  save();
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
  renderGo();
  save();
}

// One search runs at a time, because every tab shares the GPU.
function renderGo() {
  const go = $('go');
  const mine = state.searching && state.searchTab === state.active;
  go.classList.toggle('stop', mine);
  if (state.searching && !mine) {
    const other = tabById(state.searchTab);
    go.textContent = other ? `Searching ${tabName(other)}…` : 'Find plans';
    go.disabled = true;
    return;
  }
  go.textContent = mine ? 'Stop and show what was found' : 'Find plans';
  go.disabled = !mine && !state.best && mixTotal() !== 6;
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
  const t = tab();
  t.search = { rounds: [], note: 'Starting the GPU…', time: '', shown: true };
  state.plans = [];
  renderPlans();
  state.searchTab = t.id;
  setSearching(true);
  renderSearchPanel();
  try {
    await invoke('start_search', { request: {
      tab: t.id, plants: $('paste').value, mix: mixText(), neighbours: Number($('neighbours').value),
      seconds: Number($('seconds').value), ...growing(),
    } });
  } catch (error) {
    setSearching(false);
    showError(String(error));
  }
}

let searchClock = null;
const searchSpan = { end: 0, seconds: 0 };

function stopSearchClock() {
  clearInterval(searchClock);
  searchClock = null;
  $('searchClock').hidden = true;
}

// The limit is a ceiling: a search that reaches its target or runs out of routes ends sooner.
function startSearchClock(seconds) {
  stopSearchClock();
  Object.assign(searchSpan, { end: performance.now() + seconds * 1000, seconds });
  searchClock = setInterval(showSearchClock, 100);
  showSearchClock();
}

// The clock shows only on the tab that's searching.
function showSearchClock() {
  const here = state.searching && state.searchTab === state.active && searchClock;
  $('searchClock').hidden = !here;
  if (!here) return;
  const left = Math.max(0, (searchSpan.end - performance.now()) / 1000);
  const whole = Math.ceil(left);
  $('searchBar').style.width = `${100 * (1 - left / searchSpan.seconds)}%`;
  $('searchLeft').textContent = left > 0
    ? `Up to ${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')} left, less if it finishes sooner`
    : 'Time is up. Finishing the last batch…';
}

function roundItem(r) {
  const li = document.createElement('li');
  const left = document.createElement('b');
  left.textContent = `${r.crossings} crossing${r.crossings === 1 ? '' : 's'}`;
  const right = document.createElement('span');
  right.textContent = `${r.new_genotypes.toLocaleString()} new · ${r.seconds.toFixed(2)}s${r.complete ? '' : ' · cut short'}`;
  li.append(left, right);
  return li;
}

function renderSearchPanel() {
  const s = tab().search;
  $('roundsPanel').hidden = !s.shown;
  $('rounds').replaceChildren(...s.rounds.map(roundItem));
  $('searchNote').textContent = s.note;
  $('searchTime').textContent = s.time;
  showSearchClock();
}

function setSearching(on) {
  state.searching = on;
  if (!on) {
    stopSearchClock();
    state.searchTab = null;
  }
  renderGo();
  renderTabs();
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
  empty.innerHTML = '<h3>Plans appear here</h3><p>Scan or type your clones, pick the genes you want, and press Find plans.</p>';
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
  state.run = { index: plan.index, goal: plan.goal, guide: plan.guide.Ok, cue: 0, anchor: null, anchorStep: null, timerEnd: null, timerLength: null };
  applyHotkey();
  switchView('guide');
  enterCue();
  renderTabs();
}

function cue() {
  return state.run.guide.cues[state.run.cue];
}

// Estimates count from when this crossing's centre was confirmed. Without that there is none,
// and the cue's condition is all there is to go by.
function estimateEnd(c, run = state.run) {
  if (c.kind !== 'wait' || !run.anchor || run.anchorStep !== c.step) return null;
  return run.anchor + c.wait_until * 1000;
}

// Each tab has its own alarm, so guides for several crops can run at once.
function armAlarm(t = tab()) {
  const left = t.run ? (t.run.timerEnd - Date.now()) / 1000 : 0;
  if (t.run && t.run.timerEnd && left > 0) invoke('arm_alarm', { tab: t.id, seconds: left });
  else invoke('disarm_alarm', { tab: t.id });
}

function enterCue() {
  const run = state.run;
  const c = cue();
  run.timerEnd = estimateEnd(c);
  if (run.timerEnd) run.timerLength = c.wait_until * 1000;
  armAlarm();
  announce(c);
  save();
  renderGuide();
}

// Moves to any cue without confirming the current one, so a missed or mistaken cue can be fixed.
function goTo(index) {
  const run = state.run;
  if (!run || index < 0 || index >= run.guide.cues.length || index === run.cue) return;
  run.cue = index;
  run.multiplyDone = false;
  enterCue();
}

function advance() {
  const run = state.run;
  if (!run) return;
  const c = cue();
  if (c.kind === 'multiply' && !run.multiplyDone) {
    run.multiplyDone = true;
    run.timerEnd = Date.now() + c.timer_seconds * 1000;
    run.timerLength = c.timer_seconds * 1000;
    invoke('arm_alarm', { tab: tab().id, seconds: c.timer_seconds });
    speak('Timer started.');
    save();
    renderGuide();
    return;
  }
  run.multiplyDone = false;
  if (c.kind === 'plant_centre') {
    run.anchor = Date.now();
    run.anchorStep = c.step;
  }
  if (c.kind === 'finish') {
    endRun();
    return;
  }
  run.cue += 1;
  enterCue();
}

function endRun() {
  state.run = null;
  invoke('disarm_alarm', { tab: tab().id });
  applyHotkey();
  save();
  renderGuide();
  renderTabs();
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
    case 'plant_neighbour': return c.plants.length === 1 ? `${c.headline}. ${letters}.${where}` : `${c.headline}.`;
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
  // With several guides running, each cue says which crop it's for.
  speak(runningTabs().length > 1 ? `${tabName(tab())}. ${spoken(c)}` : spoken(c));
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
  box.append(checkLine());
  return box;
}

function checkLine(several) {
  const check = document.createElement('div');
  check.className = 'also';
  check.textContent = several ? 'Check the genes match before you plant each one.' : 'Check the genes match before you plant it.';
  return check;
}

// One neighbour in a neighbour cue: its planting number, which matches the planter picture,
// its genes, and where it's stored.
function neighbourRow(n, _, all) {
  const row = document.createElement('div');
  row.className = 'nrow';
  const number = document.createElement('b');
  number.textContent = n.number;
  const where = document.createElement('span');
  where.textContent = n.places.length ? placeShort(n.places) : 'made earlier in this guide';
  row.append(number, genes(n.genes, all.length < 3 ? 'big' : 'mid'), where);
  return row;
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
  if (needs) big.replaceChildren(...needs.map(needRow));
  else if ((c.neighbours || []).length) big.replaceChildren(...c.neighbours.map(neighbourRow), checkLine(c.neighbours.length > 1));
  else big.replaceChildren(...c.plants.map(p => genes(p, c.plants.length < 3)));
  const detail = document.createElement('p');
  detail.className = 'detail';
  detail.textContent = c.detail;
  card.append(eyebrow, head, big, placesBox(c.places || []));
  if (c.condition) {
    const look = document.createElement('div');
    look.className = 'condition';
    const label = document.createElement('b');
    label.textContent = c.kind === 'plant_neighbour' ? 'Deadline' : 'Look for';
    const text = document.createElement('span');
    text.textContent = c.condition;
    look.append(label, text);
    card.append(look);
  }
  card.append(detail);
  // Clicking another card jumps to it. Clicks on the live controls bubble up here too, and by then
  // the cue they confirmed has already moved on, so they're left out. The button may already be
  // gone from the page, so this checks the path the click took rather than where it is now.
  card.onclick = event => {
    if (!state.run || event.composedPath().includes($('live'))) return;
    if (Number(card.dataset.index) !== state.run.cue) goTo(Number(card.dataset.index));
  };
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
  if (!run) {
    const others = runningTabs().map(tabName);
    $('guideEmptyTitle').textContent = `No plan running for ${tabName(tab())}`;
    $('guideEmptyText').textContent = others.length
      ? `${others.join(' and ')} ${others.length === 1 ? 'has a guide' : 'have guides'} running. Pick a tab above, or start a plan for this crop on the Breed page.`
      : 'Pick a plan on the Breed page and press Start guide.';
    return;
  }
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
    case 'plant_centre': add('Centre planted', advance, true, true); break;
    case 'wait': {
      const next = run.guide.cues[run.cue + 1];
      add(next && next.kind === 'plant_neighbour' ? 'The centre is there' : 'The centre is Mature', advance, true, true);
      break;
    }
    case 'plant_neighbour': add('Planted', advance, true, true); break;
    case 'check':
      add('Genes match', advance, true, true);
      add("They don't match", () => {
        card.querySelector('.detail').textContent = "Stop here and don't take clones of this plant. Check that no other plant of this crop was nearby and that the planting order was followed, or that the server didn't restart. The crossing can be repeated from this step.";
      });
      break;
    case 'clone': add('Clones taken', advance, true, true); break;
    case 'multiply':
      add(run.multiplyDone ? 'Clones taken' : 'All planted', advance, true, true);
      break;
    case 'finish': add('End guide', advance, true, true); break;
  }
  $('prevCue').disabled = run.cue === 0;
  $('nextCue').disabled = run.cue === run.guide.cues.length - 1;
  $('cueCount').textContent = `Step ${run.cue + 1} of ${run.guide.cues.length}`;
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
  // Planter spots a cue fills. Guides saved before neighbours shared a cue have one per cue.
  const spots = x => {
    if (x.kind === 'plant_centre') return [x.slot];
    if (x.kind !== 'plant_neighbour') return [];
    return (x.neighbours || []).length ? x.neighbours.map(n => n.slot) : [x.slot];
  };
  const placed = new Set();
  for (let i = 0; i < run.cue; i++) {
    const past = run.guide.cues[i];
    if (past.step === c.step) spots(past).forEach(slot => placed.add(slot));
  }
  const box = $('planter');
  box.replaceChildren();
  let order = 0;
  const numbers = new Map();
  for (const x of run.guide.cues) if (x.step === c.step && x.kind === 'plant_neighbour') spots(x).forEach(slot => numbers.set(slot, ++order));
  const next = new Set(spots(c));
  for (let slot = 0; slot < 9; slot++) {
    const cell = document.createElement('div');
    if (slot === 4) { cell.textContent = 'centre'; cell.classList.add('centre'); }
    else if (numbers.has(slot)) cell.textContent = numbers.get(slot);
    if (placed.has(slot)) cell.classList.add('filled');
    if (next.has(slot)) cell.classList.add('next');
    box.append(cell);
  }
}

function tick() {
  updateTabs();
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
    $('timerLabel').textContent = left > 0
      ? 'Estimate only. Go by what the plant shows, not this timer.'
      : 'The estimate has passed. Check the plant, and go on only once it matches.';
  }
  const late = $('deadline');
  const overdue = c.deadline && run.anchor && run.anchorStep === c.step && now > run.anchor + c.deadline * 1000;
  late.hidden = !overdue;
  if (overdue) late.textContent = 'By the estimate, the centre could be close to Crossbreed. Check it hasn\'t reached Crossbreed before you plant more.';
}

// The estimate only prompts a look at the plant. Moving on is always the player's call. An alarm
// for another tab names its crop and marks that tab.
function alarmFired(id) {
  const t = tabById(id);
  if (!t || !t.run) return;
  const c = t.run.guide.cues[t.run.cue];
  const words = c.kind === 'wait' ? 'Check the centre plant.' : c.kind === 'multiply' ? 'Check whether the plants have reached Sapling.' : null;
  if (!words) return;
  chime();
  if (t.id === state.active) {
    speak(runningTabs().length > 1 ? `${tabName(t)}. ${words}` : words);
    renderGuide();
  } else {
    t.attention = true;
    speak(`${tabName(t)}. ${words}`);
    updateTabs();
  }
}

// F9 confirms the current cue of the guide on screen, and stays claimed while any guide runs.
function applyHotkey() {
  invoke('set_hotkey', { on: runningTabs().length > 0 && $('optHotkey').checked }).catch(error => {
    $('optHotkey').checked = false;
    showError(`F9 is in use by another app: ${error}`);
  });
}

/* ---------- grow times ---------- */

async function renderTimes() {
  const { fertilized, fullWater } = growing();
  const tables = await invoke('grow_table', { fertilized, fullWater });
  $('timesNote').textContent = `Minutes from planting, in a planter ${fertilized ? 'with' : 'without'} fertiliser, ${fullWater ? 'kept full of water' : 'kept near its ideal water level'}, with perfect light and heat. Plants move in 60-second ticks, each up to 10% early or late.`;
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

// Health as the game shows it: the lowest of light, water, ground and temperature. Light and
// heat are taken as perfect. A planter's ground is full only with fertiliser, a full planter
// waters at 0.6, and both are measured against what the crop wants.
function renderHealth() {
  const { crop, fertilized, fullWater } = growing();
  const info = state.crops.find(c => c.key === crop);
  if (!info) return;
  const ground = Math.min(1, (fertilized ? 1 : 0.6) / info.optimal_ground);
  const water = fullWater ? Math.min(1, 0.6 / info.optimal_water) : 1;
  const health = Math.min(ground, water);
  for (const b of document.querySelectorAll('[data-water]')) b.classList.toggle('active', (b.dataset.water === 'full') === fullWater);
  $('health').textContent = `${info.name} should show ${Math.round(health * 100)}% health: ground ${Math.round(ground * 100)}%${fertilized ? '' : ' without fertiliser'}, water ${Math.round(water * 100)}%${fullWater ? ' in a full planter' : ''}, with light and heat taken as perfect. Timings use this.`;
}

// A running guide takes new timings for the same plan. A wait in progress moves its end time.
function refreshRun(t = tab()) {
  const run = t.run;
  if (!run) return;
  const plan = t.plans.find(p => p.index === run.index && p.goal === run.goal) || t.plans.find(p => p.goal === run.goal);
  const guide = plan && plan.guide.Ok;
  if (!guide || guide.cues.length !== run.guide.cues.length) return;
  run.guide = guide;
  const c = guide.cues[run.cue];
  if (c.kind === 'wait') {
    run.timerEnd = estimateEnd(c, run);
    if (run.timerEnd) run.timerLength = c.wait_until * 1000;
    armAlarm(t);
  }
  save();
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
  state.searchTab = null;
  $('bench').disabled = true;
  renderGo();
  $('benchNote').textContent = 'Measuring. This takes about 10 seconds…';
}

function endBenchmark(text) {
  state.searching = false;
  $('bench').disabled = false;
  renderMix(); // restores Find plans, which needs a complete target
  $('benchNote').textContent = text;
}

// Fertiliser and water apply to every tab. The crop belongs to the tab showing.
async function replan(everyTab) {
  save();
  renderHealth();
  renderTabs();
  await readPaste();
  if (!$('view-times').hidden) renderTimes();
  for (const t of everyTab ? state.tabs : [tab()]) {
    if (!t.plans.length) continue;
    t.plans = await invoke('replan', { tab: t.id, ...growing(t) });
    refreshRun(t);
  }
  renderPlans();
  renderGuide();
}

async function main() {
  const crops = await invoke('crops');
  state.crops = crops;
  for (const crop of crops) $('crop').add(new Option(crop.name, crop.key));
  const add = $('addCrop');
  add.add(new Option('+ Add crop', ''));
  for (const crop of crops) add.add(new Option(crop.name, crop.key));
  add.onchange = () => { if (add.value) addTab(add.value); add.value = ''; };
  load();
  renderTabs();
  renderHealth();
  renderMix();
  renderSearchPanel();
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
  $('crop').onchange = () => { tab().crop = $('crop').value; replan(false); };
  $('fertilized').onchange = () => replan(true);
  for (const b of document.querySelectorAll('[data-water]')) {
    b.onclick = () => { state.fullWater = b.dataset.water === 'full'; replan(true); };
  }
  for (const id of ['neighbours', 'seconds']) $(id).onchange = save;
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
  await listen('alarm', ({ payload }) => alarmFired(payload));
  await listen('hotkey-next', advance);
  await listen('search-started', ({ payload: seconds }) => startSearchClock(seconds));
  // Search results belong to the tab that started the search, even if another is showing now.
  await listen('search-round', ({ payload: r }) => {
    const t = tabById(state.searchTab);
    if (!t) return;
    t.search.rounds.push(r);
    t.search.note = `${r.known.toLocaleString()} plants reachable so far. Trying ${r.crossings + 1} crossings…`;
    if (t.id === state.active) renderSearchPanel();
  });
  await listen('benchmark-done', ({ payload: b }) => endBenchmark(
    `Top speed ${compact(b.best)} groups/s, median ${compact(b.median)}, over ${b.runs} runs on ${b.device}. ` +
    'Every machine runs the same 100-clone work, so results compare.'));
  await listen('benchmark-failed', ({ payload }) => endBenchmark(`Benchmark failed: ${payload}`));
  await listen('search-failed', ({ payload }) => {
    const t = tabById(state.searchTab);
    setSearching(false);
    if (!t) return;
    t.search.shown = false;
    if (t.id === state.active) {
      renderSearchPanel();
      showError(payload);
    } else {
      t.search.note = payload;
    }
  });
  await listen('search-done', ({ payload: r }) => {
    const t = tabById(state.searchTab);
    setSearching(false);
    if (!t) return;
    const s = t.search;
    if (r.already_have) {
      s.note = `You already have ${r.already_have}, which meets this target.`;
    } else {
      t.plans = r.plans;
      s.time = `${r.seconds.toFixed(1)}s on ${r.device}`;
      const n = r.proven_up_to;
      const span = `${n} crossing${n === 1 ? '' : 's'}`;
      const best = r.plans.length ? r.plans[0].crossings : 0;
      const proven = best > 0 && best <= n + 1 && !r.stopped;
      if (r.reached_goal) {
        s.note = proven ? `This is the minimum: every route up to ${span} was searched at this neighbour limit.` : 'A shorter route may exist, because the search ended early.';
      } else {
        const ruled = n ? `No route within ${span} at this neighbour limit. ` : '';
        s.note = `Target not reached. ${ruled}Try a longer search or more neighbours.`;
      }
    }
    if (t.id === state.active) {
      renderSearchPanel();
      renderPlans();
    }
    renderTabs();
  });

  $('prevCue').onclick = () => state.run && goTo(state.run.cue - 1);
  $('nextCue').onclick = () => state.run && goTo(state.run.cue + 1);
  document.addEventListener('keydown', event => {
    if (!state.run || $('view-guide').hidden || event.target.closest?.('input, textarea, select')) return;
    if (event.key === 'PageUp') { event.preventDefault(); goTo(state.run.cue - 1); }
    if (event.key === 'PageDown') { event.preventDefault(); goTo(state.run.cue + 1); }
  });

  for (const t of runningTabs()) {
    // Guides saved before estimates were tied to a crossing belong to the current one.
    if (t.run.anchor && t.run.anchorStep === undefined) t.run.anchorStep = t.run.guide.cues[t.run.cue].step;
    armAlarm(t);
  }
  if (runningTabs().length) {
    applyHotkey();
    switchView('guide');
  }
  setInterval(tick, 250);
}

main().catch(error => showError(String(error)));
