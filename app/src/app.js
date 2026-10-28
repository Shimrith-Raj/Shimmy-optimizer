'use strict';
const $ = (s, r = document) => r.querySelector(s);
const { invoke } = window.__TAURI__.core;
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const fmt = (mb) => (Math.abs(mb) >= 1024 ? (mb / 1024).toFixed(1) + ' GB' : Math.round(mb) + ' MB');
const REDUCED = matchMedia('(prefers-reduced-motion: reduce)').matches;

const S = { state: null, history: [], range: 3600000, open: new Set(), filter: '', startup: null, hoverX: null, tab: 'home', segs: [], shownPct: 0, cpuHist: [], cpuRange: 3600000, cpuHoverX: null, gpuHist: [], gpuRange: 300000, gpuHoverX: null };

// v0.2 called an HTTP API; Shimmy calls Rust commands. Same paths kept so the rest reads as before.
const COMMANDS = {
  '/api/state': 'get_state', '/api/history': 'get_history', '/api/log': 'get_log', '/api/startup': 'get_startup',
  '/api/gpu': 'get_gpu', '/api/storage': 'get_storage', '/api/usage/scan': 'start_usage_scan', '/api/folder': 'storage_folder',
  '/api/open': 'open_path', '/api/clean': 'get_clean', '/api/clean/scan': 'start_clean_scan', '/api/clean/run': 'start_clean',
  '/api/battery': 'get_battery', '/api/settings': 'get_settings', '/api/settings/save': 'set_settings', '/api/autostart': 'set_autostart', '/api/plugged': 'set_plugged_mode', '/api/caches/scan': 'start_cache_scan', '/api/caches/run': 'start_cache_clean', '/api/folder/preview': 'folder_preview', '/api/folder/delete': 'folder_delete', '/api/bin/empty': 'empty_bin', '/api/hibernation': 'set_hibernation', '/api/restore': 'get_restore_points', '/api/apps-settings': 'open_apps_settings', '/api/charge-limit': 'set_charge_limit', '/api/fan': 'set_fan_mode', '/api/heat': 'set_heat_protection', '/api/dism': 'start_dism', '/api/finder': 'start_finder', '/api/finder/delete': 'finder_delete', '/api/startup/toggle': 'toggle_startup', '/api/close': 'close_procs', '/api/trim': 'trim', '/api/quickclean': 'quick_clean',
};
async function api(path, body) {
  try {
    return await invoke(COMMANDS[path], body || {});
  } catch (e) {
    throw new Error(String(e));
  }
}

function toast(msg, err) {
  const t = document.createElement('div');
  t.className = 'toast' + (err ? ' err' : '');
  t.textContent = msg;
  $('#toasts').append(t);
  setTimeout(() => t.remove(), err ? 8000 : 6000);
}

// ---------- Hero ----------

function countTo(el, to) {
  const from = S.shownPct;
  S.shownPct = to;
  if (REDUCED || from === to) { el.textContent = to; return; }
  const t0 = performance.now();
  const step = (t) => {
    const k = Math.min(1, (t - t0) / 700);
    el.textContent = Math.round(from + (to - from) * (1 - Math.pow(1 - k, 3)));
    if (k < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

// ---------- Home ----------

const LEVELS = { good: 0, warn: 1, bad: 2 };
const levelOf = (v, warn, bad) => (v >= bad ? 'bad' : v >= warn ? 'warn' : 'good');

function tile(id, big, sub, level) {
  const t = $(id);
  t.className = 'tile ' + (level || '');
  t.querySelector('.big').innerHTML = big;
  t.querySelector('.sub').textContent = sub;
}

function renderHome() {
  const { mem, home, recs } = S.state;
  const ram = levelOf(mem.pct, 70, 85);
  const cpu = S.state.cpu.tempC != null ? S.state.cpu.status.level : levelOf(home.cpuPct, 70, 90);
  const disk = home.diskFreeGb < 10 ? 'bad' : home.diskFreeGb < 20 ? 'warn' : 'good';
  const b = home.battery;
  const bat = !b.present || b.plugged || b.pct == null ? 'good' : b.pct <= 10 ? 'bad' : b.pct <= 20 ? 'warn' : 'good';
  tile('#tRam', `${mem.pct}<small>%</small>`, `${fmt(mem.usedMB)} of ${fmt(mem.totalMB)}`, ram);
  const c = S.state.cpu;
  if (c.tempC != null) tile('#tCpu', `${Math.round(c.tempC)}<small>°C</small>`, `${Math.round(home.cpuPct)}% load${c.avgClockMhz ? `, ${(c.avgClockMhz / 1000).toFixed(1)} GHz` : ''}`, c.status.level);
  else tile('#tCpu', `${Math.round(home.cpuPct)}<small>%</small>`, 'Load. Temperature needs admin rights', cpu);
  tile('#tDisk', `${Math.round(home.diskFreeGb)}<small>GB</small>`, `of ${Math.round(home.diskTotalGb)} GB`, disk);
  if (b.present) {
    const left = b.secondsLeft ? `, about ${hm(b.secondsLeft / 60)} left` : '';
    const health = S.state.batteryHealth != null ? `, health ${S.state.batteryHealth}%` : '';
    const held = S.state.plugged?.on && b.plugged;
    tile('#tBat', `${b.pct ?? '–'}<small>%</small>`, (held ? `Running from the charger, held at ${S.state.plugged.holdPct}%` : b.plugged ? 'Plugged in' : 'On battery' + left) + health, bat);
  } else tile('#tBat', '–', 'No battery found', '');

  const worst = [[ram, 'Memory is getting tight.', 'Memory is under pressure.'], [disk, 'C: is filling up.', 'C: is almost full.'],
    [cpu, 'The processor is running hot.', 'The processor is overheating.'], [bat, 'Battery is low.', 'Battery is very low.']]
    .filter(([l]) => l !== 'good').sort((x, y) => LEVELS[y[0]] - LEVELS[x[0]])[0];
  $('#verdict').textContent = worst ? (worst[0] === 'bad' ? worst[2] : worst[1]) : 'Your PC is in good shape.';
  const top = recs.find((r) => r.action === 'close' && r.savingsMB > 0);
  $('#verdictSub').textContent = top ? `Biggest win right now: ${top.title.replace(/^Close /, 'closing ')}, about ${fmt(top.savingsMB)}.` : 'Nothing is worth closing right now.';
}

function renderHero() {
  const m = S.state.mem;
  const level = m.pct >= 85 ? 'bad' : m.pct >= 70 ? 'warn' : 'good';
  document.body.dataset.level = level;
  countTo($('#pct'), m.pct);
  $('#statusTxt').textContent = { good: 'Healthy.', warn: 'Getting tight.', bad: 'Under pressure.' }[level];
  $('#usedTxt').textContent = `${fmt(m.usedMB)} of ${fmt(m.totalMB)} in use, ${fmt(m.freeMB)} free.`;
  if (S.state.selfMB != null) $('#selfLine').textContent = `Shimmy itself is using ${fmt(S.state.selfMB)}, this window included. With the window closed it keeps only its tray and sensors running.`;
}

// ---------- Memory map: RAM drawn as cells, coloured by the apps holding them ----------

const CELLS = 160;

function renderMap() {
  const { mem, groups } = S.state;
  const per = mem.totalMB / CELLS;
  const usedCells = Math.min(CELLS, Math.round(mem.usedMB / per));
  // Working sets double-count shared pages, so the top apps can add up to more than "used". Scale them down to fit.
  const top = groups.slice(0, 6);
  const scale = Math.min(1, (mem.usedMB * 0.92) / Math.max(1, top.reduce((a, g) => a + g.mb, 0)));
  let left = usedCells;
  let hue = 0;
  const segs = top.map((g) => {
    const n = Math.min(left, Math.max(1, Math.round((g.mb * scale) / per)));
    left -= n;
    return { app: g.app, mb: g.mb, count: g.count, n, c: g.app === 'Windows system' ? 5 : [1, 2, 3, 4, 6][hue++] };
  }).filter((s) => s.n > 0);
  if (left > 0) segs.push({ app: 'Everything else', mb: left * per, n: left, c: 0 });
  S.segs = segs;

  const map = $('#map');
  const first = !map.children.length;
  if (first) {
    map.innerHTML = '<i></i>'.repeat(CELLS);
    if (!REDUCED) map.classList.add('boot');
  }
  const cells = map.children;
  let k = 0;
  segs.forEach((s, si) => {
    for (let j = 0; j < s.n; j++, k++) { cells[k].className = 'c' + s.c; cells[k].dataset.c = si; }
  });
  for (; k < CELLS; k++) { cells[k].className = ''; delete cells[k].dataset.c; }
  if (first && !REDUCED) {
    [...cells].forEach((c, i) => { c.style.transitionDelay = (i % 40) * 8 + Math.floor(i / 40) * 30 + 'ms'; });
    requestAnimationFrame(() => requestAnimationFrame(() => {
      map.classList.remove('boot');
      setTimeout(() => [...cells].forEach((c) => (c.style.transitionDelay = '')), 1200);
    }));
  }

  const freeCells = CELLS - usedCells;
  $('#legend').innerHTML = segs.map((s, i) =>
    `<button data-seg="${i}"><i class="c${s.c}"></i>${esc(s.app)} <span>${fmt(s.mb)}</span></button>`).join('')
    + `<button data-seg="free" tabindex="-1"><i style="background:var(--m-free)"></i>Free <span>${fmt(mem.freeMB)} · ${freeCells} cells</span></button>`;
}

function focusSeg(i) {
  S.focus = i;
  const map = $('#map');
  document.querySelectorAll('.legend button').forEach((b) => b.classList.toggle('hi', b.dataset.seg === String(i)));
  if (i == null || i === 'free') {
    map.classList.remove('focus');
    [...map.children].forEach((c) => c.classList.remove('hi'));
    $('#mapCaption').textContent = i === 'free'
      ? `${fmt(S.state.mem.freeMB)} free, ready for whatever you open next.`
      : 'Each cell is one slice of your RAM. Point at a cell to see who holds it.';
    return;
  }
  const s = S.segs[i];
  map.classList.add('focus');
  [...map.children].forEach((c) => c.classList.toggle('hi', c.dataset.c === String(i)));
  $('#mapCaption').innerHTML = s.c === 0
    ? `<b>Everything else</b> holds ${fmt(s.mb)}: smaller apps, Windows caches and the kernel.`
    : `<b>${esc(s.app)}</b> holds ${fmt(s.mb)} across ${s.count} process${s.count === 1 ? '' : 'es'}. Click to inspect.`;
}

// ---------- Chart ----------

function drawChart() {
  const c = $('#chart');
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = c.clientHeight;
  if (!w) return;
  c.width = w * dpr; c.height = h * dpr;
  const g = c.getContext('2d');
  g.scale(dpr, dpr);
  const css = getComputedStyle(document.body);
  const col = (n) => css.getPropertyValue(n).trim();
  const total = S.state?.mem.totalMB || 16384;
  const L = 64, R = 20, T = 24, B = 34;
  const now = Date.now(), from = now - S.range;
  const pts = S.history.filter((p) => p[0] >= from);
  const X = (t) => L + ((t - from) / S.range) * (w - L - R);
  const Y = (v) => T + (1 - v / total) * (h - T - B);
  const font = getComputedStyle(document.documentElement).fontFamily;

  g.font = `13px ${font}`;
  g.fillStyle = col('--muted');
  g.textAlign = 'right';
  for (let i = 0; i <= 4; i++) {
    const v = (total / 4) * i;
    g.fillText((v / 1024).toFixed(0) + ' GB', L - 14, Y(v) + 4);
  }
  // The 85% line marks "under pressure", drawn as a filled band rather than a rule.
  g.fillStyle = 'rgba(255,77,61,.07)';
  g.fillRect(L, T, w - L - R, Y(total * 0.85) - T);
  g.fillStyle = col('--muted');
  g.textAlign = 'center';
  for (let i = 0; i <= 4; i++) {
    const t = from + (S.range / 4) * i;
    g.fillText(new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }), Math.min(Math.max(X(t), L + 20), w - R - 20), h - 10);
  }
  if (pts.length < 2) { g.fillText('Collecting data', w / 2, h / 2); return; }

  const line = new Path2D();
  pts.forEach((p, i) => (i ? line.lineTo(X(p[0]), Y(p[1])) : line.moveTo(X(p[0]), Y(p[1]))));
  const area = new Path2D(line);
  area.lineTo(X(pts[pts.length - 1][0]), Y(0)); area.lineTo(X(pts[0][0]), Y(0)); area.closePath();
  g.fillStyle = 'rgba(198,244,50,.09)'; g.fill(area);
  g.strokeStyle = col('--accent'); g.lineWidth = 2.5; g.lineJoin = 'round'; g.stroke(line);

  const last = pts[pts.length - 1];
  g.fillStyle = col('--accent'); g.fillRect(X(last[0]) - 4, Y(last[1]) - 4, 8, 8);

  if (S.hoverX != null && S.hoverX > L) {
    const t = from + ((S.hoverX - L) / (w - L - R)) * S.range;
    let best = pts[0];
    for (const p of pts) if (Math.abs(p[0] - t) < Math.abs(best[0] - t)) best = p;
    const x = X(best[0]), y = Y(best[1]);
    g.fillStyle = 'rgba(238,240,242,.35)'; g.fillRect(x - 0.5, T, 1.5, h - T - B);
    g.fillStyle = col('--ink'); g.fillRect(x - 5, y - 5, 10, 10);
    g.font = `600 14px ${font}`;
    const label = `${fmt(best[1])}  ${new Date(best[0]).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
    const tw = g.measureText(label).width + 20;
    const tx = Math.min(Math.max(x - tw / 2, L), w - R - tw);
    g.fillStyle = col('--ink'); g.fillRect(tx, 2, tw, 26);
    g.fillStyle = col('--on-accent'); g.textAlign = 'left'; g.fillText(label, tx + 10, 20);
  }
}

// ---------- Suggestions ----------

function renderRecs() {
  const recs = S.state.recs;
  const total = recs.filter((r) => r.action === 'close').reduce((a, r) => a + r.savingsMB, 0);
  $('#recSummary').textContent = recs.length ? `${recs.length} found, up to ${fmt(total)} you could get back` : '';
  if (!recs.length) {
    $('#recs').innerHTML = '<div class="empty">Nothing worth closing right now. Your memory is in good shape.</div>';
    return;
  }
  $('#recs').innerHTML = recs.map((r, i) => {
    const risk = r.action === 'info' ? '<span class="risk tip">Tip</span>' : `<span class="risk ${r.risk}">${r.risk === 'low' ? 'Safe' : 'Review first'}</span>`;
    const btn = r.action === 'close' ? `<button class="btn small ghost" data-rec="${i}">Review and close</button>`
      : r.action === 'trim' ? '<button class="btn small ghost" data-trim>Trim memory</button>' : '';
    const save = r.savingsMB ? `<div class="save">${fmt(r.savingsMB)}</div>` : '<div class="save none">Advice</div>';
    return `<div class="rec">${save}<div><div class="t">${esc(r.title)}${risk}</div><p class="w">${esc(r.why)}</p></div><div class="act">${btn}</div></div>`;
  }).join('');
}

// ---------- Processor ----------

const ghz = (mhz) => (mhz / 1000).toFixed(1);

function renderCpu() {
  const c = S.state.cpu;
  $('#cpuBigWrap').className = 'pct lvl-' + c.status.level;
  $('#cpuBig').textContent = c.tempC != null ? Math.round(c.tempC) : Math.round(c.loadPct);
  $('#cpuUnit').textContent = c.tempC != null ? '°C' : '%';
  $('#cpuStatus').textContent = c.status.text + (S.state.heat?.boostOff ? ' Boost is off to cool it down.' : '');
  renderFan();
  const [fan, ...otherFans] = c.fans;
  const facts = [
    [`${Math.round(c.loadPct)}%`, 'Load'],
    [c.avgClockMhz ? `${ghz(c.avgClockMhz)} GHz` : '–', c.baseMhz ? `Clock, rated ${ghz(c.baseMhz)} GHz` : 'Clock'],
    [c.packageW != null ? `${c.packageW.toFixed(1)} W` : '–', 'Power draw'],
    [fan ? `${fan.rpm.toLocaleString()} rpm` : '–', fan ? [fan.name, ...otherFans.map((f) => `${f.name.toLowerCase()} ${f.rpm.toLocaleString()} rpm`)].join(', ') : 'Fans not reported'],
  ];
  $('#cpuFacts').innerHTML = facts.map(([v, l]) => `<div><b>${esc(v)}</b><span>${esc(l)}</span></div>`).join('');
  const note = $('#sensorNote');
  note.hidden = c.sensorsLive && c.tempC != null;
  if (!c.sensorsLive) note.textContent = `Temperature, real clocks and power need Shimmy to run as administrator with the PawnIO driver installed.${c.sensorsError ? ` (${c.sensorsError})` : ''}`;
  else if (c.tempC == null) note.textContent = "The processor's temperature sensor is not answering right now, so load is shown instead. Another monitoring app (such as an ASUS utility) may be holding it; Shimmy keeps trying.";

  $('#cpuName').textContent = c.name || 'Cores';
  $('#coreCaption').textContent = `${c.cores.length} cores, ${c.threads} threads. The shading is how busy each core is.`;
  // Core tiles are built once and then only updated, so live refreshes slide the bars instead of replaying an animation.
  const wrap = $('#cores');
  if (wrap.children.length !== c.cores.length) wrap.innerHTML = c.cores.map(() => '<div class="core"><i class="fill"></i><b></b><span></span></div>').join('');
  c.cores.forEach((k, i) => {
    const el = wrap.children[i];
    el.classList.toggle('busy', k.loadPct >= 60);
    el.querySelector('.fill').style.height = Math.min(100, k.loadPct) + '%';
    el.querySelector('b').innerHTML = k.clockMhz ? `${ghz(k.clockMhz)}<small>GHz</small>` : `${Math.round(k.loadPct)}<small>%</small>`;
    el.querySelector('span').textContent = k.clockMhz ? `${Math.round(k.loadPct)}% busy${k.powerW != null ? `, ${k.powerW.toFixed(1)} W` : ''}` : 'busy';
  });

  const top = S.state.groups.filter((g) => g.cpu >= 0.5).sort((a, b) => b.cpu - a.cpu).slice(0, 6);
  $('#cpuTop').innerHTML = top.length ? top.map((g) => {
    const act = g.protected ? `<span class="lock">Protected, ${esc(g.reason)}</span>` : `<button class="btn small ghost" data-close-app="${esc(g.app)}">Close</button>`;
    return `<div class="rec"><div class="save">${g.cpu.toFixed(1)}%</div><div><div class="t">${esc(g.app)}</div><p class="w">${g.count} process${g.count === 1 ? '' : 'es'}, ${fmt(g.mb)} of memory</p></div><div class="act">${act}</div></div>`;
  }).join('') : '<div class="empty">Nothing is using the processor right now.</div>';
  drawCpuChart();
}

const FAN_MODES = [
  ['silent', 'Silent', 'Quietest. The firmware also lowers the processor\'s power limit, so it runs cooler and slower.'],
  ['standard', 'Standard', 'The everyday balance of speed, heat and noise.'],
  ['performance', 'Performance', 'Full power, with the fans at their loudest.'],
];

function renderFan() {
  const { fanMode, heat } = S.state;
  const fan = S.state.cpu.fans[0];
  $('#fanCaption').textContent = fan ? `${fan.name} at ${fan.rpm.toLocaleString()} rpm` : 'Fn+F switches the same modes';
  const modes = fanMode
    ? `<div class="limits">${FAN_MODES.map(([id, name, why]) => `<button data-fan="${id}" class="${fanMode === id ? 'on' : ''}"><b>${name}</b><span>${why}</span></button>`).join('')}</div>`
    : '<p class="care-note">This laptop\'s firmware does not offer fan modes through the route Shimmy uses.</p>';
  const since = heat.since ? ` since ${new Date(heat.since).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : '';
  const text = !heat.enabled
    ? 'Off. Boost stays on whatever the temperature.'
    : heat.boostOff
      ? `Boost is off${since}: cooling down. It comes back after 2 minutes at ${heat.coolC} °C or less.`
      : `On. If the processor stays at ${heat.hotC} °C or more for 30 seconds, Shimmy turns boost off until it cools to ${heat.coolC} °C. Usually several degrees cooler for a small loss of speed.`;
  $('#fanCtl').innerHTML = `<div class="hold${heat.boostOff ? ' on' : ''}"><div><b>Heat protection</b><p>${esc(text)}</p></div>${sw('data-heat', heat.enabled, 'Heat protection')}</div>${modes}
    <p class="care-note">Fn+F and MyASUS change the same setting; Shimmy shows whichever mode is active. The fans themselves follow the firmware's curve for the mode, which cannot be changed on this laptop.</p>`;
}

async function loadGpu() {
  try {
    const r = await api('/api/gpu');
    const g = r.gpu;
    $('#gpu').innerHTML = g ? [
      `<div class="gname"><b>${esc(g.name)}</b><span>${g.loadPct != null ? `${g.loadPct}% busy` : ''}</span></div>`,
      `<div><b>${g.tempC ?? '–'}${g.tempC != null ? ' °C' : ''}</b><span>Temperature</span></div>`,
      `<div><b>${g.vramUsedMb != null ? fmt(g.vramUsedMb) : '–'}</b><span>${g.vramTotalMb ? `of ${fmt(g.vramTotalMb)} video memory` : 'Video memory'}</span></div>`,
      `<div><b>${g.powerW != null ? g.powerW.toFixed(1) + ' W' : '–'}</b><span>Power draw</span></div>`,
    ].join('') : `<div class="none"><b>No NVIDIA graphics found</b><span>${esc(r.error || '')}</span></div>`;
  } catch (e) {
    $('#gpu').innerHTML = `<div class="none"><b>Graphics unavailable</b><span>${esc(e.message)}</span></div>`;
  }
}

function drawCpuChart() {
  drawTempLoad($('#cpuChart'), S.cpuHist, S.cpuRange, S.cpuHoverX, 95);
}

function drawGpuChart() {
  drawTempLoad($('#gpuChart'), S.gpuHist, S.gpuRange, S.gpuHoverX, S.gpuHot || 92);
}

// Temperature (accent) and load (ink) over time, for the Processor and Graphics pages. `hot` starts the red band.
function drawTempLoad(c, hist, range, hoverX, hot) {
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = c.clientHeight;
  if (!w) return;
  c.width = w * dpr; c.height = h * dpr;
  const g = c.getContext('2d');
  g.scale(dpr, dpr);
  const css = getComputedStyle(document.body);
  const col = (n) => css.getPropertyValue(n).trim();
  const font = getComputedStyle(document.documentElement).fontFamily;
  const L = 64, R = 20, T = 24, B = 34, now = Date.now(), from = now - range;
  const pts = hist.filter((p) => p[0] >= from);
  const X = (t) => L + ((t - from) / range) * (w - L - R);
  const Y = (v) => T + (1 - Math.min(v, 100) / 100) * (h - T - B);
  g.font = `13px ${font}`; g.fillStyle = col('--muted'); g.textAlign = 'right';
  for (let v = 0; v <= 100; v += 25) g.fillText(String(v), L - 14, Y(v) + 4);
  // Where the chip starts to throttle (95 °C for the Ryzen), shown as a band, not a rule.
  g.fillStyle = 'rgba(255,77,61,.07)'; g.fillRect(L, T, w - L - R, Y(hot) - T);
  g.fillStyle = col('--muted'); g.textAlign = 'center';
  for (let i = 0; i <= 4; i++) {
    const t = from + (range / 4) * i;
    g.fillText(new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }), Math.min(Math.max(X(t), L + 20), w - R - 20), h - 10);
  }
  if (pts.length < 2) { g.fillText('Collecting data', w / 2, h / 2); return; }
  const line = (i, color, width) => {
    const path = new Path2D();
    let started = false;
    for (const p of pts) {
      if (p[i] == null) { started = false; continue; }
      if (started) path.lineTo(X(p[0]), Y(p[i])); else path.moveTo(X(p[0]), Y(p[i]));
      started = true;
    }
    g.strokeStyle = color; g.lineWidth = width; g.lineJoin = 'round'; g.stroke(path);
  };
  line(1, col('--ink'), 1.5);
  line(2, col('--accent'), 2.5);
  if (hoverX != null && hoverX > L) {
    const t = from + ((hoverX - L) / (w - L - R)) * range;
    let best = pts[0];
    for (const p of pts) if (Math.abs(p[0] - t) < Math.abs(best[0] - t)) best = p;
    const x = X(best[0]);
    g.fillStyle = 'rgba(238,240,242,.35)'; g.fillRect(x - 0.5, T, 1.5, h - T - B);
    g.font = `600 14px ${font}`;
    const label = `${best[2] != null ? Math.round(best[2]) + ' °C  ' : ''}${Math.round(best[1])}% load  ${new Date(best[0]).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
    const tw = g.measureText(label).width + 20;
    const tx = Math.min(Math.max(x - tw / 2, L), w - R - tw);
    g.fillStyle = col('--ink'); g.fillRect(tx, 2, tw, 26);
    g.fillStyle = col('--on-accent'); g.textAlign = 'left'; g.fillText(label, tx + 10, 20);
  }
}

// ---------- Storage ----------

const gb = (b) => b / 1073741824;
const fmtB = (b) => (b >= 1073741824 ? `${gb(b).toFixed(1)} GB` : b >= 1048576 ? `${Math.round(b / 1048576)} MB` : b >= 1024 ? `${Math.round(b / 1024)} KB` : `${b} bytes`);
const ago = (ms) => {
  const m = Math.round((Date.now() - ms) / 60000);
  return m < 1 ? 'just now' : m < 60 ? `${m} min ago` : `${Math.round(m / 60)} h ago`;
};
const USAGE_COLORS = { yours: 'var(--m1)', appdata: 'var(--m3)', apps: 'var(--m2)', shared: 'var(--m4)', system: 'var(--m5)' };

let jobTimer = null;
// While any scan or clean-up runs, poll once a second; the 5 s refresh covers the rest.
function pollSoon() {
  clearTimeout(jobTimer);
  jobTimer = setTimeout(() => (S.tab === 'storage' ? loadStorage() : S.tab === 'clean' ? loadClean() : null), 1000);
}

async function loadStorage() {
  let st;
  try { st = await api('/api/storage'); } catch (e) { return toast(e.message, true); }
  S.storage = st;
  const c = st.drives.find((d) => d.letter.toUpperCase() === 'C:') || st.drives[0];
  if (c) {
    const freeGb = gb(c.free);
    $('#diskBigWrap').className = 'pct lvl-' + (freeGb < 10 ? 'bad' : freeGb < 20 ? 'warn' : 'good');
    $('#diskBig').textContent = Math.round(freeGb);
    $('#diskSub').textContent = `Free on C:, of ${Math.round(gb(c.total))} GB. ${Math.round((1 - c.free / c.total) * 100)}% is in use.`;
  }
  const h = st.health[0];
  $('#diskFacts').innerHTML = (h ? [
    [h.lifePct != null ? `${Math.round(h.lifePct)}%` : '–', 'SSD life left'],
    [h.tempC != null ? `${Math.round(h.tempC)} °C` : '–', h.warnC ? `Temperature, warns at ${Math.round(h.warnC)} °C` : 'Temperature'],
    [h.writtenGb != null ? `${(h.writtenGb / 1024).toFixed(1)} TB` : '–', 'Written so far'],
    [h.sparePct != null ? `${Math.round(h.sparePct)}%` : '–', 'Spare blocks left'],
  ] : [['–', 'Drive health needs Shimmy to run as administrator']]).map(([v, l], _, all) => `<div${all.length === 1 ? ' style="grid-column:1/-1"' : ''}><b>${esc(v)}</b><span>${esc(l)}</span></div>`).join('');

  const others = st.drives.filter((d) => d !== c);
  $('#otherDrives').hidden = !others.length;
  $('#driveList').innerHTML = others.map((d) => `<div class="frow"><b>${esc(d.letter)} ${esc(d.label)}</b><span class="num">${fmtB(d.free)} free</span><span class="bar-col"><span class="meter"><i style="width:${(1 - d.free / d.total) * 100}%"></i></span></span><span class="muted">of ${fmtB(d.total)}</span></div>`).join('');

  const job = st.usage;
  $('#usageBtn').disabled = job.running;
  $('#usageBtn').textContent = job.running ? 'Measuring' : job.result ? 'Measure again' : 'Measure C:';
  if (job.running) {
    $('#usageNote').textContent = `${job.progress}…`;
    pollSoon();
  } else if (job.error) {
    $('#usageNote').textContent = job.error;
  }
  const u = job.result;
  if (u) {
    const free = u.total - u.used;
    const segs = [...u.categories.map((k) => ({ ...k, color: USAGE_COLORS[k.id] })), { id: 'free', name: 'Free', bytes: free, color: 'var(--m-free)' }];
    $('#usageBar').innerHTML = segs.map((k) => `<i style="flex-basis:${(k.bytes / u.total) * 100}%;background:${k.color}" title="${esc(k.name)}, ${fmtB(k.bytes)}"></i>`).join('');
    $('#usageLegend').innerHTML = segs.map((k) => `<button data-ucat="${k.id}" title="${k.id === 'free' ? '' : 'Show it'}"><i style="background:${k.color}"></i>${esc(k.name)} <span>${fmtB(k.bytes)}</span></button>`).join('');
    if (!job.running) $('#usageNote').textContent = `Measured ${u.files.toLocaleString()} files ${ago(job.at)}. Windows and system also counts the page file, the hibernation file and other user accounts.`;
    if (!S.folderLoaded) loadFolder(null);
  } else if (!job.running) {
    $('#folders').innerHTML = '<div class="empty">Measure C: to see which of your folders are biggest.</div>';
  }
  renderCaches(st);
  renderSys(st);
}

async function loadFolder(path) {
  const f = await api('/api/folder', { path });
  if (!f) {
    $('#folders').innerHTML = '<div class="empty">Measure C: to see which folders are biggest.</div>';
    return;
  }
  S.folderLoaded = true;
  const profile = S.storage.profile;
  const rel = f.path.length > profile.length ? f.path.slice(profile.length + 1).split('\\') : [];
  const crumbs = [{ name: 'Your folder', path: profile }];
  rel.forEach((name, i) => crumbs.push({ name, path: profile + '\\' + rel.slice(0, i + 1).join('\\') }));
  $('#crumbs').innerHTML = crumbs.map((c) => `<button data-folder="${esc(c.path)}">${esc(c.name)}</button>`).join('');
  const max = Math.max(1, ...f.children.map((k) => k.bytes), f.filesHere);
  const rows = f.children.map((k) => `<div class="frow drill" data-folder="${esc(k.path)}"><b>${esc(k.name)}</b><span class="num">${fmtB(k.bytes)}</span><span class="bar-col"><span class="meter"><i style="width:${(k.bytes / max) * 100}%"></i></span></span><span class="row-acts"><button class="btn small ghost" data-open="${esc(k.path)}">Show</button><button class="btn small ghost" data-del="${esc(k.path)}">Delete</button></span></div>`);
  if (f.filesHere) rows.push(`<div class="frow"><b class="muted">Files directly in this folder</b><span class="num">${fmtB(f.filesHere)}</span><span class="bar-col"><span class="meter"><i style="width:${(f.filesHere / max) * 100}%"></i></span></span><button class="btn small ghost" data-open="${esc(f.path)}">Show</button></div>`);
  $('#folders').innerHTML = rows.join('') || '<div class="empty">This folder is empty.</div>';
}


// ---------- Storage: deleting, old caches, system files ----------

async function previewDelete(path) {
  const name = path.split('\\').pop();
  $('#modalBody').innerHTML = `<h3>Checking ${esc(name)}</h3><div class="muted">Counting what is inside…</div>`;
  $('#modal').showModal();
  let r;
  try { r = await api('/api/folder/preview', { path }); } catch (e) { $('#modal').close(); return toast(e.message, true); }
  if (r.refused) {
    $('#modalBody').innerHTML = `<h3>Shimmy won’t delete ${esc(name)}</h3><div class="warn-box">It is ${esc(r.refused)}.</div>
      <div class="modal-actions"><button class="btn ghost" id="mCancel">OK</button></div>`;
    $('#mCancel').onclick = () => $('#modal').close();
    return;
  }
  const c = r.contents;
  const fits = c.bytes <= r.binCapacity;
  const binNote = 'Goes to the Recycle Bin: you can restore it, and the space is freed once the bin is emptied.';
  $('#modalBody').innerHTML = `<h3>Delete ${esc(name)}?</h3><div class="muted">${esc(path)}</div>
    <div class="plist"><div><span>Size</span><span>${fmtB(c.bytes)}</span></div><div><span>Files</span><span>${c.files.toLocaleString()}</span></div><div><span>Folders inside</span><span>${c.folders.toLocaleString()}</span></div></div>
    ${fits ? '' : `<div class="warn-box">This is bigger than your Recycle Bin (${fmtB(r.binCapacity)}), so Windows can only delete it for good.</div>`}
    <label class="perm"><input type="checkbox" class="tick" id="mPerm" ${fits ? '' : 'checked disabled'}>Delete for good and free ${fmtB(c.bytes)} now (no undo)</label>
    <div class="muted" id="mNote">${fits ? binNote : 'It cannot be restored afterwards.'}</div>
    <div class="modal-actions"><button class="btn ghost" id="mCancel">Cancel</button><button class="btn danger" id="mOk">${fits ? 'Move to Recycle Bin' : 'Delete for good'}</button></div>`;
  $('#mCancel').onclick = () => $('#modal').close();
  $('#mPerm').onchange = () => {
    const perm = $('#mPerm').checked;
    $('#mOk').textContent = perm ? 'Delete for good' : 'Move to Recycle Bin';
    $('#mNote').textContent = perm ? 'Deleted straight away. It cannot be restored.' : binNote;
  };
  $('#mOk').onclick = async () => {
    $('#mOk').disabled = true; $('#mOk').textContent = 'Working';
    try {
      const d = await api('/api/folder/delete', { path, permanent: $('#mPerm').checked, allowBig: !fits });
      $('#modal').close();
      if (d.needsConfirm) return toast('The folder grew past the Recycle Bin’s size. Check it again.', true);
      toast(d.permanent ? `Deleted ${name} for good. ${fmtB(d.bytes)} freed.` : `Moved ${name} to the Recycle Bin (${fmtB(d.bytes)}). Empty the bin to free the space.`);
      loadFolder(path.split('\\').slice(0, -1).join('\\'));
      loadStorage();
    } catch (e) { $('#modal').close(); toast(e.message, true); }
  };
}

function renderCaches(st) {
  const j = st.caches, run = st.cacheRun;
  const list = j.result || [];
  if (!j.result && !j.running && !S.cacheAsked) {
    S.cacheAsked = true;
    api('/api/caches/scan').then(pollSoon);
  }
  if (!S.cacheSel && list.length) S.cacheSel = new Set(list.filter((k) => k.defaultOn && !k.blocked && k.old).map((k) => k.id));
  const sel = S.cacheSel || new Set();
  const busy = j.running || run.running;
  if (busy) pollSoon();
  $('#cacheBtn').disabled = busy;
  $('#cacheBtn').textContent = j.running ? 'Measuring' : 'Measure again';
  $('#cacheLead').textContent = `Files untouched for ${st.cacheAgeDays} days or more (you can change that in Settings). They go to the Recycle Bin, so the space is freed once it is emptied. A cache is skipped while its app is open; the cache folders themselves stay.`;
  $('#cacheList').innerHTML = !list.length && j.running ? `<div class="empty">${esc(j.progress)}…</div>`
    : list.map((k) => {
      const off = k.blocked || !k.old;
      return `<label class="cat${off ? ' off' : ''}"><input type="checkbox" class="tick" data-cache="${k.id}" ${sel.has(k.id) && !off ? 'checked' : ''} ${off ? 'disabled' : ''}>
        <span class="save${k.old ? '' : ' none'}">${k.old ? fmtB(k.old) : 'Nothing old'}</span>
        <span><span class="t">${esc(k.title)}</span> <span class="muted">${esc(k.family)}, ${fmtB(k.total)} in all</span><p class="w">${esc(k.why)}</p>${k.blocked ? `<p class="blocked">${esc(k.blocked)}</p>` : ''}</span></label>`;
    }).join('') || (j.result ? '<div class="empty">No app caches found on this PC.</div>' : '');
  const total = list.filter((k) => sel.has(k.id) && !k.blocked).reduce((a, k) => a + k.old, 0);
  S.cacheTotal = total;
  $('#cacheBar').hidden = !total && !run.running;
  $('#cacheSel').textContent = run.running ? `${run.progress}…` : `${fmtB(total)} of old cache files ticked`;
  $('#cacheGo').disabled = busy || !total;
  if (S.cacheWaiting && run.result && !run.running) {
    S.cacheWaiting = false;
    const moved = run.result.reduce((a, r) => a + r.recycled, 0);
    const notes = run.result.filter((r) => r.note).map((r) => `${r.title}: ${r.note}`);
    toast(`Moved ${fmtB(moved)} of old cache files to the Recycle Bin.${notes.length ? ' ' + notes.join(' ') : ''}`);
  }
}

function confirmCaches() {
  const st = S.storage;
  const picked = (st.caches.result || []).filter((k) => S.cacheSel.has(k.id) && !k.blocked && k.old);
  const room = Math.max(0, st.bin.capacity - st.bin.bytes);
  $('#modalBody').innerHTML = `<h3>Move ${fmtB(S.cacheTotal)} of old cache files to the Recycle Bin?</h3>
    <div class="muted">Only files untouched for ${st.cacheAgeDays} days or more. The apps rebuild what they need.</div>
    <div class="plist">${picked.map((k) => `<div><span>${esc(k.title)}</span><span>${fmtB(k.old)}</span></div>`).join('')}</div>
    ${S.cacheTotal > room ? `<div class="warn-box">The Recycle Bin has room for about ${fmtB(room)} more. To fit these, Windows removes the oldest things already in the bin for good, and anything that still does not fit is deleted for good.</div>` : ''}
    <div class="modal-actions"><button class="btn ghost" id="mCancel">Cancel</button><button class="btn danger" id="mOk">Move to Recycle Bin</button></div>`;
  $('#modal').showModal();
  $('#mCancel').onclick = () => $('#modal').close();
  $('#mOk').onclick = async () => {
    $('#modal').close();
    S.cacheWaiting = true;
    await api('/api/caches/run', { ids: picked.map((k) => k.id) });
    loadStorage();
  };
}

function renderSys(st) {
  const b = st.bin, apps = st.usage.result?.categories.find((k) => k.id === 'apps');
  const row = (title, sub, size, act) => `<div class="frow sys"><b>${esc(title)}<span class="where">${esc(sub)}</span></b><span class="num">${size}</span><span></span>${act}</div>`;
  $('#sysFiles').innerHTML = [
    row('Recycle Bin', `${b.items.toLocaleString()} item${b.items === 1 ? '' : 's'}. Holds up to ${fmtB(b.capacity)} on C:.`, fmtB(b.bytes), `<button class="btn small ghost" id="binEmpty" ${b.items ? '' : 'disabled'}>Empty</button>`),
    row('Hibernation file', st.hiberBytes ? 'Lets the laptop hibernate and start faster from off (Fast Startup).' : 'Off. Hibernate and Fast Startup are turned off.', st.hiberBytes ? fmtB(st.hiberBytes) : 'Off', sw('data-hiber', st.hiberBytes > 0, 'Hibernation')),
    row('Restore points', 'Managed by Windows. Shown here only.', S.restore ? esc(S.restore) : '–', ''),
    row('Installed apps', 'Uninstall from Windows’ own Installed apps page.', apps ? fmtB(apps.bytes) : '–', '<button class="btn small ghost" id="appsOpen">Open</button>'),
  ].join('');
  if (!S.restoreAsked) {
    S.restoreAsked = true;
    api('/api/restore').then((r) => { S.restore = r.known ? r.used || 'None' : 'Unknown'; if (S.tab === 'storage') renderSys(S.storage); }).catch(() => {});
  }
}

function confirmEmptyBin() {
  const b = S.storage.bin;
  $('#modalBody').innerHTML = `<h3>Empty the Recycle Bin?</h3><div class="muted">${b.items.toLocaleString()} items, ${fmtB(b.bytes)}. They are deleted for good and cannot be restored.</div>
    <div class="modal-actions"><button class="btn ghost" id="mCancel">Cancel</button><button class="btn danger" id="mOk">Empty it</button></div>`;
  $('#modal').showModal();
  $('#mCancel').onclick = () => $('#modal').close();
  $('#mOk').onclick = async () => {
    $('#mOk').disabled = true; $('#mOk').textContent = 'Working';
    try { const r = await api('/api/bin/empty'); toast(`Recycle Bin emptied. ${fmtB(r.bytes)} freed.`); } catch (e) { toast(e.message, true); }
    $('#modal').close();
    loadStorage();
  };
}

function confirmHibernation(input) {
  const on = input.checked;
  if (on) return setHibernation(true, input);
  input.checked = true;
  $('#modalBody').innerHTML = `<h3>Turn hibernation off?</h3>
    <div class="muted">Frees ${fmtB(S.storage.hiberBytes)} on C:. Sleep keeps working. You lose Hibernate and Fast Startup, so starting from fully off takes a little longer. You can turn it back on here at any time.</div>
    <div class="modal-actions"><button class="btn ghost" id="mCancel">Cancel</button><button class="btn danger" id="mOk">Turn off</button></div>`;
  $('#modal').showModal();
  $('#mCancel').onclick = () => $('#modal').close();
  $('#mOk').onclick = () => { $('#modal').close(); setHibernation(false, input); };
}

async function setHibernation(on, input) {
  input.disabled = true;
  try {
    const r = await api('/api/hibernation', { on });
    toast(on ? 'Hibernation is on again.' : `Hibernation is off. ${fmtB(S.storage.hiberBytes - r.bytes)} freed.`);
  } catch (e) { toast(e.message, true); }
  loadStorage();
}

// ---------- Clean ----------

S.cleanSel = null;
S.finderSel = new Set();
S.finderAt = 0;

async function loadClean() {
  let c;
  try { c = await api('/api/clean'); } catch (e) { return toast(e.message, true); }
  const busy = c.scan.running || c.run.running || c.dism.running || c.finder.running;
  if (!c.scan.result && !c.scan.running && !c.scan.error) {
    await api('/api/clean/scan');
    return pollSoon();
  }
  renderCats(c);
  renderDism(c.dism);
  renderFinder(c.finder);
  if (S.cleanWasRunning && !c.run.running && c.run.result) {
    const r = c.run.result;
    const freed = r.reduce((a, x) => a + x.freed, 0);
    toast(`Cleaned ${fmtB(freed)}.` + (r.some((x) => x.note) ? ' ' + r.filter((x) => x.note).map((x) => `${x.title}: ${x.note}`).join(' ') : ''));
  }
  S.cleanWasRunning = c.run.running;
  if (busy) pollSoon();
}

function renderCats(c) {
  const cats = c.scan.result || [];
  if (!S.cleanSel && cats.length) S.cleanSel = new Set(cats.filter((k) => k.defaultOn && !k.blocked).map((k) => k.id));
  const sel = S.cleanSel || new Set();
  const total = cats.filter((k) => sel.has(k.id) && !k.blocked).reduce((a, k) => a + k.bytes, 0);
  const [num, unit] = total >= 1073741824 ? [gb(total).toFixed(1), 'GB'] : [Math.round(total / 1048576), 'MB'];
  $('#cleanBig').textContent = c.scan.result ? num : '–';
  $('#cleanUnit').textContent = c.scan.result ? unit : '';
  $('#cleanSub').textContent = c.run.running ? `${c.run.progress}…` : c.scan.running ? `${c.scan.progress}…` : 'Ready to clean from what is ticked below.';
  $('#cleanRun').disabled = c.run.running || c.scan.running || !total;
  $('#cleanRun').textContent = c.run.running ? 'Cleaning' : 'Clean selected';
  $('#cleanRescan').disabled = c.run.running || c.scan.running;
  $('#cats').innerHTML = cats.map((k) => `<label class="cat ${k.blocked ? 'off' : ''}">
      <input type="checkbox" class="tick" data-cat="${k.id}" ${sel.has(k.id) && !k.blocked ? 'checked' : ''} ${k.blocked ? 'disabled' : ''}>
      <span class="save${k.bytes ? '' : ' none'}">${k.bytes ? fmtB(k.bytes) : 'Empty'}</span>
      <span><span class="t">${esc(k.title)}</span><p class="w">${esc(k.why)}</p>${k.blocked ? `<p class="blocked">${esc(k.blocked)}</p>` : ''}</span>
    </label>`).join('');
}

function renderDism(j) {
  const d = j.result;
  let text, btn = '';
  if (j.running) text = `${j.progress}…`;
  else if (j.error) { text = j.error; btn = '<button class="btn ghost" data-dism="analyse">Try again</button>'; }
  else if (!d) { text = 'Windows keeps backups of old updates so they can be uninstalled. Check how much space they take.'; btn = '<button class="btn ghost" data-dism="analyse">Check</button>'; }
  else {
    text = d.summary + (d.storeSize ? ` The component store holds ${d.storeSize}.` : '');
    btn = d.recommended && !d.cleaned ? '<button class="btn" data-dism="cleanup">Clean up</button>' : '<button class="btn ghost" data-dism="analyse">Check again</button>';
  }
  $('#dism').innerHTML = `<p>${esc(text)}</p><div>${btn}</div>`;
}

function fileRow(i) {
  return `<label class="frow file"><input type="checkbox" class="tick" data-file="${esc(i.path)}" data-bytes="${i.bytes}" ${S.finderSel.has(i.path) ? 'checked' : ''}>
    <b>${esc(i.name)}<span class="where">${esc(i.folder)}</span></b><span class="num">${fmtB(i.bytes)}</span>
    <span class="muted when">${new Date(i.modified).toLocaleDateString()}</span><button class="btn small ghost" data-open="${esc(i.path)}">Show</button></label>`;
}

function renderFinder(j) {
  $('#finderBtn').disabled = j.running;
  $('#finderBtn').textContent = j.running ? 'Looking' : j.result ? 'Look again' : 'Look for them';
  if (j.running) { $('#finder').innerHTML = `<div class="empty">${esc(j.progress)}…</div>`; return; }
  if (j.error) { $('#finder').innerHTML = `<div class="empty">${esc(j.error)}</div>`; return; }
  const f = j.result;
  if (!f || (j.at === S.finderAt && $('#finder').children.length)) return;
  S.finderAt = j.at;
  const wasted = f.dups.reduce((a, g) => a + g.bytes * (g.files.length - 1), 0);
  const parts = [];
  parts.push(`<div class="fgroup"><h3>Duplicates${f.dups.length ? `: ${fmtB(wasted)} in extra copies` : ''}</h3>${f.dups.length ? f.dups.map((g) => `<div class="dupe"><p>${g.files.length} identical copies, ${fmtB(g.bytes)} each. The first is the oldest, most likely the original.</p><div class="folders">${g.files.map(fileRow).join('')}</div></div>`).join('') : '<div class="empty">No duplicates found.</div>'}</div>`);
  parts.push(`<div class="fgroup"><h3>Files over 500 MB</h3>${f.large.length ? `<div class="folders">${f.large.map(fileRow).join('')}</div>` : '<div class="empty">None found.</div>'}</div>`);
  parts.push(`<div class="fgroup"><h3>Installers in Downloads older than two months</h3>${f.installers.length ? `<div class="folders">${f.installers.map(fileRow).join('')}</div>` : '<div class="empty">None found.</div>'}</div>`);
  parts.push(`<p class="map-caption">Looked at ${f.scanned.toLocaleString()} files ${ago(j.at)}.${f.skippedProjects.length ? ` Skipped ${f.skippedProjects.length} code project folder${f.skippedProjects.length === 1 ? '' : 's'}.` : ''}</p>`);
  $('#finder').innerHTML = parts.join('');
  updateFinderBar();
}

function updateFinderBar() {
  const boxes = [...document.querySelectorAll('#finder input[data-file]')].filter((b) => S.finderSel.has(b.dataset.file));
  const bytes = [...new Map(boxes.map((b) => [b.dataset.file, Number(b.dataset.bytes)])).values()].reduce((a, b) => a + b, 0);
  $('#finderBar').hidden = !S.finderSel.size;
  $('#finderSel').textContent = `${S.finderSel.size} file${S.finderSel.size === 1 ? '' : 's'} selected, ${fmtB(bytes)}`;
}

function confirmDelete(paths) {
  const list = paths.map((p) => `<div><span>${esc(p.split('\\').pop())}</span><span class="muted">${esc(p.split('\\').slice(-2, -1)[0] || '')}</span></div>`).join('');
  $('#modalBody').innerHTML = `<h3>Move ${paths.length} file${paths.length === 1 ? '' : 's'} to the Recycle Bin?</h3>
    <div class="muted">You can restore them from the Recycle Bin until it is emptied.</div><div class="plist">${list}</div>
    <div class="modal-actions"><button class="btn ghost" id="mCancel">Cancel</button><button class="btn danger" id="mOk">Move to Recycle Bin</button></div>`;
  $('#modal').showModal();
  $('#mCancel').onclick = () => $('#modal').close();
  $('#mOk').onclick = () => doDelete(paths, false);
}

async function doDelete(paths, allowPermanent) {
  $('#mOk').disabled = true; $('#mOk').textContent = 'Working';
  try {
    const r = await api('/api/finder/delete', { paths, allowPermanent });
    if (r.needsConfirm) {
      const big = r.needsConfirm;
      const fits = paths.filter((p) => !big.some((b) => b.path === p));
      $('#modalBody').innerHTML = `<h3>${big.length} file${big.length === 1 ? ' is' : 's are'} too big for the Recycle Bin</h3>
        <div class="warn-box">Windows would delete ${big.length === 1 ? 'it' : 'them'} <b>permanently</b>, with no way to restore.</div>
        <div class="plist">${big.map((b) => `<div><span>${esc(b.path.split('\\').pop())}</span><span>${fmtB(b.bytes)}</span></div>`).join('')}</div>
        <div class="modal-actions"><button class="btn ghost" id="mSkip">${fits.length ? 'Skip these, recycle the rest' : 'Cancel'}</button><button class="btn danger" id="mOk">Delete permanently</button></div>`;
      $('#mSkip').onclick = () => (fits.length ? doDelete(fits, false) : $('#modal').close());
      $('#mOk').onclick = () => doDelete(paths, true);
      return;
    }
    $('#modal').close();
    paths.forEach((p) => S.finderSel.delete(p));
    const freed = r.recycledBytes + r.permanentBytes;
    toast(`${r.recycled ? `Moved ${r.recycled} to the Recycle Bin. ` : ''}${r.permanent ? `Deleted ${r.permanent} permanently. ` : ''}${fmtB(freed)} freed.${r.refused.length ? ` Refused ${r.refused.length}: ${r.refused.map((x) => x.reason).join(', ')}.` : ''}${r.failed.length ? ` ${r.failed.length} could not be removed.` : ''}`);
    S.finderAt = 0;
    loadClean();
  } catch (e) {
    $('#modal').close();
    toast(e.message, true);
  }
}

// ---------- Battery ----------

const wh = (mwh) => `${(mwh / 1000).toFixed(1)} Wh`;
const hm = (min) => { const m = Math.round(min); return `${Math.floor(m / 60)} h ${m % 60} min`; };

async function loadBattery() {
  let r;
  try { r = await api('/api/battery'); } catch (e) { return toast(e.message, true); }
  const b = r.battery;
  S.battery = b;
  if (!b.present) {
    $('#batBig').textContent = '–';
    $('#batStatus').textContent = 'No battery found. This PC runs on mains power only.';
    return;
  }
  $('#batBigWrap').className = 'pct lvl-' + b.status.level;
  $('#batBig').textContent = b.healthPct != null ? Math.round(b.healthPct) : '–';
  $('#batStatus').textContent = b.status.text;

  const rate = b.rateW != null && Math.abs(b.rateW) >= 0.5 ? b.rateW : null;
  const nowLabel = b.plugged
    ? (rate > 0 ? `Charging at ${rate.toFixed(0)} W` : b.charging ? 'Charging' : 'Plugged in, not charging')
    : `On battery${b.secondsLeft ? `, ${hm(b.secondsLeft / 60)} left` : ''}${rate < 0 ? `, using ${(-rate).toFixed(0)} W` : ''}`;
  $('#batFacts').innerHTML = [
    [b.chargePct != null ? `${Math.round(b.chargePct)}%` : '–', nowLabel],
    [b.cycles != null ? String(b.cycles) : '–', 'Charge cycles'],
    [b.fullMwh ? wh(b.fullMwh) : '–', b.designMwh ? `Full charge now, of ${wh(b.designMwh)} when new` : 'Full charge now'],
    [b.fullRuntimeMin ? hm(b.fullRuntimeMin) : '–', 'Windows’ estimate on a full charge'],
  ].map(([v, l]) => `<div><b>${esc(v)}</b><span>${esc(l)}</span></div>`).join('');

  const h = b.history;
  $('#wearCaption').textContent = h.length > 1 ? `${h[0].date} to ${h[h.length - 1].date}, from Windows' battery report` : '';
  drawWearChart();
  S.powerHist = r.powerHistory;
  renderPower(r.power, b, r.hold);
  renderCare(b, r.hold);
  if (r.error) toast(r.error, true);
}

function renderPower(p, b, hold) {
  const w = (v) => `${Math.abs(v) < 0.5 ? 0 : Math.round(v)} W`;
  const bat = p.batteryW == null ? ['–', 'Battery'] : p.batteryW >= 0.5 ? [`+${w(p.batteryW)}`, 'Going into the battery'] : p.batteryW <= -0.5 ? [`−${w(-p.batteryW)}`, 'Coming out of the battery'] : ['0 W', 'Battery resting'];
  const facts = [
    bat,
    [p.cpuW != null ? `${p.cpuW.toFixed(1)} W` : '–', 'Processor'],
    !p.gpuPresent ? ['None', 'No dedicated graphics card'] : p.gpuAwake ? [p.gpuW != null ? `${p.gpuW.toFixed(1)} W` : '–', 'Graphics card, in use'] : ['Asleep', 'Graphics card, about 0 W'],
    [p.totalW != null ? `${p.totalExact ? '' : 'about '}${Math.round(p.totalW)} W` : '–', p.totalExact ? 'Whole laptop, measured from the battery' : 'Whole laptop, estimated (no charger sensor)'],
  ];
  $('#powerFacts').innerHTML = facts.map(([v, l]) => `<div><b>${esc(v)}</b><span>${esc(l)}</span></div>`).join('');
  const note = $('#drainNote');
  note.hidden = !p.draining;
  if (p.draining) {
    const out = Math.round(-p.batteryW);
    note.textContent = hold?.on
      ? `Run from the charger is on, but the battery is being used: ${out} W are coming out of it because the charger cannot keep up with this load. It stops once the load drops.`
      : `Plugged in, but ${out} W are coming out of the battery: the charger cannot keep up with this load.`;
  }
  drawPowerChart();
}

// Battery watts (accent; above 0 charging, below 0 draining) and processor watts (ink) over the last hour. Stretches
// where the battery drained while plugged in are shaded red.
function drawPowerChart() {
  const c = $('#powerChart');
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = c.clientHeight;
  if (!w) return;
  c.width = w * dpr; c.height = h * dpr;
  const g = c.getContext('2d');
  g.scale(dpr, dpr);
  const css = getComputedStyle(document.body);
  const col = (n) => css.getPropertyValue(n).trim();
  const font = getComputedStyle(document.documentElement).fontFamily;
  const L = 64, R = 20, T = 24, B = 34, range = 3600000, now = Date.now(), from = now - range;
  const pts = (S.powerHist || []).filter((p) => p[0] >= from);
  const vals = pts.flatMap((p) => [p[1], p[2]]).filter((v) => v != null);
  const top = Math.max(20, Math.ceil(Math.max(0, ...vals) / 10) * 10);
  const bottom = Math.min(0, Math.floor(Math.min(0, ...vals) / 10) * 10);
  const X = (t) => L + ((t - from) / range) * (w - L - R);
  const Y = (v) => T + (1 - (v - bottom) / (top - bottom)) * (h - T - B);
  g.font = `13px ${font}`; g.fillStyle = col('--muted'); g.textAlign = 'right';
  const step = Math.max(10, Math.ceil((top - bottom) / 4 / 10) * 10);
  for (let v = bottom; v <= top; v += step) g.fillText(`${v} W`, L - 14, Y(v) + 4);
  g.textAlign = 'center';
  for (let i = 0; i <= 4; i++) {
    const t = from + (range / 4) * i;
    g.fillText(new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }), Math.min(Math.max(X(t), L + 20), w - R - 20), h - 10);
  }
  if (pts.length < 2) { g.fillText('Collecting data', w / 2, h / 2); return; }
  g.fillStyle = 'rgba(255,77,61,.18)';
  for (let i = 0; i < pts.length - 1; i++) {
    const p = pts[i];
    if (p[3] && p[1] != null && p[1] <= -0.5) g.fillRect(X(p[0]), T, Math.max(1, X(pts[i + 1][0]) - X(p[0])), h - T - B);
  }
  g.fillStyle = 'rgba(238,240,242,.25)'; g.fillRect(L, Y(0), w - L - R, 1);
  const line = (i, color, width) => {
    const path = new Path2D();
    let started = false;
    for (const p of pts) {
      if (p[i] == null) { started = false; continue; }
      if (started) path.lineTo(X(p[0]), Y(p[i])); else path.moveTo(X(p[0]), Y(p[i]));
      started = true;
    }
    g.strokeStyle = color; g.lineWidth = width; g.lineJoin = 'round'; g.stroke(path);
  };
  line(2, col('--ink'), 1.5);
  line(1, col('--accent'), 2.5);
  if (S.powerHoverX != null && S.powerHoverX > L) {
    const t = from + ((S.powerHoverX - L) / (w - L - R)) * range;
    let best = pts[0];
    for (const p of pts) if (Math.abs(p[0] - t) < Math.abs(best[0] - t)) best = p;
    const x = X(best[0]);
    g.fillStyle = 'rgba(238,240,242,.35)'; g.fillRect(x - 0.5, T, 1.5, h - T - B);
    g.font = `600 14px ${font}`;
    const bw = best[1] == null ? 'battery –' : best[1] >= 0.5 ? `battery +${Math.round(best[1])} W` : best[1] <= -0.5 ? `battery −${Math.round(-best[1])} W` : 'battery 0 W';
    const label = `${bw}  ${best[2] != null ? `processor ${Math.round(best[2])} W  ` : ''}${best[3] ? 'plugged in' : 'on battery'}  ${new Date(best[0]).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
    const tw = g.measureText(label).width + 20;
    const tx = Math.min(Math.max(x - tw / 2, L), w - R - tw);
    g.fillStyle = col('--ink'); g.fillRect(tx, 2, tw, 26);
    g.fillStyle = col('--on-accent'); g.textAlign = 'left'; g.fillText(label, tx + 10, 20);
  }
}

function renderCare(b, hold) {
  const opts = [[60, 'Plugged in most of the day'], [80, 'Balanced. Good for everyday use'], [100, 'Full charge, for a day away from power']];
  const held = !!hold?.on;
  const disabled = b.limitSupported && !held ? '' : 'disabled';
  let warn = '';
  if (held && hold.overridden) warn = `MyASUS keeps changing the charge limit, so the battery charges past ${hold.holdPct}%. In MyASUS, set Battery Care to the same value or turn it off.`;
  else if (held && hold.myasus != null && hold.myasus !== hold.holdPct) warn = `MyASUS Battery Care is set to ${hold.myasus}%. If it writes that back (it can after a restart), Shimmy sees the battery charging and puts the hold back.`;
  const holdHtml = `<div class="hold${held ? ' on' : ''}"><div><b>Run from the charger</b><p>${esc(hold?.text || '')}</p></div>${sw('data-hold', held, 'Run from the charger', !b.limitSupported)}</div>${warn ? `<p class="warn-box">${esc(warn)}</p>` : ''}`;
  let note;
  if (!b.limitSupported) note = 'This laptop’s firmware does not offer a charge limit through the route Shimmy uses. MyASUS has the same setting under Battery Care.';
  else if (b.chargeLimit && b.plugged && b.chargePct >= b.chargeLimit - 2 && !b.charging) note = `Charging has stopped at ${Math.round(b.chargePct)}%. The ${b.chargeLimit}% limit is working.`;
  else note = `${b.chargeLimit ? `Set to ${b.chargeLimit}%. ` : 'Not set from Shimmy yet. '}The firmware cannot report the limit back, so Shimmy remembers your choice and applies it again at start-up and every 30 minutes. If Battery Care is on in MyASUS, set it to the same value there, or the two will keep changing it.`;
  if (held) note = 'Your choice here comes back when you turn Run from the charger off.';
  const ok = note.startsWith('Charging has stopped');
  $('#care').innerHTML = holdHtml + `<div class="limits${held ? ' dim' : ''}">${opts.map(([v, l]) => `<button data-limit="${v}" class="${b.chargeLimit === v ? 'on' : ''}" ${disabled}><b>${v}%</b><span>${l}</span></button>`).join('')}</div>
    <p class="care-note${ok ? ' ok' : ''}">${esc(note)}</p>`;
}

async function setLimit(pct) {
  try {
    await api('/api/charge-limit', { pct });
    toast(`Battery will stop charging at ${pct}%.`);
  } catch (e) { toast(e.message, true); }
  loadBattery();
}

function drawWearChart() {
  const c = $('#wearChart');
  const h = S.battery?.history || [];
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, ht = c.clientHeight;
  if (!w) return;
  c.width = w * dpr; c.height = ht * dpr;
  const g = c.getContext('2d');
  g.scale(dpr, dpr);
  const css = getComputedStyle(document.body);
  const col = (n) => css.getPropertyValue(n).trim();
  const font = getComputedStyle(document.documentElement).fontFamily;
  g.font = `13px ${font}`; g.fillStyle = col('--muted');
  if (h.length < 2) { g.textAlign = 'center'; g.fillText('No capacity history from Windows yet', w / 2, ht / 2); return; }
  const pts = h.map((p) => [Date.parse(p.date), (p.fullMwh / p.designMwh) * 100]);
  const lo = Math.max(0, Math.floor((Math.min(...pts.map((p) => p[1])) - 5) / 10) * 10);
  const L = 64, R = 24, T = 24, B = 34;
  const t0 = pts[0][0], t1 = pts[pts.length - 1][0];
  const X = (t) => L + ((t - t0) / Math.max(1, t1 - t0)) * (w - L - R);
  const Y = (v) => T + (1 - (v - lo) / (100 - lo)) * (ht - T - B);
  g.textAlign = 'right';
  for (let v = lo; v <= 100; v += 10) g.fillText(`${v}%`, L - 14, Y(v) + 4);
  // Below 80% of design capacity is where batteries count as worn; shown as a band.
  if (lo < 80) { g.fillStyle = 'rgba(255,176,32,.07)'; g.fillRect(L, Y(80), w - L - R, Y(lo) - Y(80)); }
  g.fillStyle = col('--muted'); g.textAlign = 'center';
  for (let i = 0; i <= 4; i++) {
    const t = t0 + ((t1 - t0) / 4) * i;
    g.fillText(new Date(t).toLocaleDateString([], { month: 'short', year: 'numeric' }), Math.min(Math.max(X(t), L + 30), w - R - 30), ht - 10);
  }
  const path = new Path2D();
  pts.forEach((p, i) => (i ? path.lineTo(X(p[0]), Y(p[1])) : path.moveTo(X(p[0]), Y(p[1]))));
  g.strokeStyle = col('--accent'); g.lineWidth = 2.5; g.lineJoin = 'round'; g.stroke(path);
  g.fillStyle = col('--accent');
  const last = pts[pts.length - 1];
  g.fillRect(X(last[0]) - 4, Y(last[1]) - 4, 8, 8);
  if (S.wearHoverX != null && S.wearHoverX > L) {
    const t = t0 + ((S.wearHoverX - L) / (w - L - R)) * (t1 - t0);
    let i = 0;
    pts.forEach((p, k) => { if (Math.abs(p[0] - t) < Math.abs(pts[i][0] - t)) i = k; });
    const x = X(pts[i][0]);
    g.fillStyle = 'rgba(238,240,242,.35)'; g.fillRect(x - 0.5, T, 1.5, ht - T - B);
    g.font = `600 14px ${font}`;
    const e = h[i];
    const label = `${Math.round(pts[i][1])}%  ${wh(e.fullMwh)}${e.cycles != null ? `  ${e.cycles} cycles` : ''}  ${new Date(pts[i][0]).toLocaleDateString()}`;
    const tw = g.measureText(label).width + 20;
    const tx = Math.min(Math.max(x - tw / 2, L), w - R - tw);
    g.fillStyle = col('--ink'); g.fillRect(tx, 2, tw, 26);
    g.fillStyle = col('--on-accent'); g.textAlign = 'left'; g.fillText(label, tx + 10, 20);
  }
}

// ---------- Apps ----------

function renderApps() {
  const q = S.filter.toLowerCase();
  const groups = S.state.groups.filter((g) => !q || g.app.toLowerCase().includes(q) || g.procs.some((p) => p.name.toLowerCase().includes(q) || p.title.toLowerCase().includes(q)));
  const max = Math.max(1, ...groups.map((g) => g.mb));
  const rows = [];
  for (const g of groups) {
    const open = S.open.has(g.app);
    const ids = g.procs.filter((p) => !p.protected).map((p) => p.pid);
    const lock = g.protected ? `<span class="lock" title="This is never offered for closing">Protected, ${esc(g.reason)}</span>` : g.reason ? '<span class="lock" title="Protected processes in this group are skipped">Partly protected</span>' : '';
    rows.push(`<tr class="group ${open ? 'open' : ''}" data-app="${esc(g.app)}"><td><span class="caret">▸</span></td><td><b>${esc(g.app)}</b>${lock}</td><td class="num">${g.count}</td><td class="num">${fmt(g.mb)}</td><td class="bar-col"><div class="meter"><i style="width:${(g.mb / max) * 100}%"></i></div></td><td class="num">${ids.length ? `<button class="btn small ghost" data-close-app="${esc(g.app)}">Close</button>` : ''}</td></tr>`);
    if (open) for (const p of g.procs) {
      rows.push(`<tr class="child"><td></td><td>${esc(p.name)} <span class="muted">pid ${p.pid}</span>${p.title ? `, ${esc(p.title)}` : ''}${p.cmd ? `<div class="cmd">${esc(p.cmd)}</div>` : ''}</td><td></td><td class="num">${p.mb} MB</td><td class="bar-col"></td><td class="num">${p.protected ? `<span class="lock">Protected, ${esc(p.reason)}</span>` : `<button class="btn small ghost" data-close-pid="${p.pid}" data-app="${esc(g.app)}">Close</button>`}</td></tr>`);
    }
  }
  $('#appsTable tbody').innerHTML = rows.join('') || '<tr><td colspan="6" class="muted">No matches.</td></tr>';
}

// ---------- Close flow ----------

function procRows(list) {
  const shown = list.slice(0, 12).map((p) => `<div><span>${esc(p.name)} <span class="muted">pid ${p.pid}</span></span><span>${p.mb} MB</span></div>`).join('');
  return shown + (list.length > 12 ? `<div class="muted"><span>And ${list.length - 12} more</span><span>${list.slice(12).reduce((a, p) => a + p.mb, 0)} MB</span></div>` : '');
}

function openClose(label, list, mode = 'auto', note) {
  if (!list.length) return;
  const total = list.reduce((a, p) => a + p.mb, 0);
  const windowed = list.some((p) => p.title);
  const isBrowser = /Brave|Chrome|Edge/.test(label);
  $('#modalBody').innerHTML = `
    <h3>Close ${esc(label)}?</h3>
    <div class="muted">${list.length} process${list.length > 1 ? 'es' : ''}, about ${fmt(total)}</div>
    <div class="plist">${procRows(list)}</div>
    ${mode === 'force' ? '<div class="warn-box"><b>Force close.</b> The process is ended immediately. Anything unsaved in it is lost.</div>'
      : windowed ? `<div class="warn-box">This asks the app to close normally first${isBrowser ? ', which closes <b>all its windows and tabs</b>' : ''}. If it prompts to save, answer it there. Save your work first.</div>`
      : '<div class="warn-box">These are background processes with no window, so they are ended directly. Anything they hold in memory is lost.</div>'}
    ${note ? `<div class="muted">${esc(note)}</div>` : ''}
    <div class="modal-actions"><button class="btn ghost" id="mCancel">Cancel</button><button class="btn danger" id="mOk">${mode === 'force' ? 'Force close' : 'Close'}</button></div>`;
  $('#modal').showModal();
  $('#mCancel').onclick = () => $('#modal').close();
  $('#mOk').onclick = async () => {
    $('#mOk').disabled = true; $('#mOk').textContent = 'Working';
    try {
      const r = await api('/api/close', { pids: list.map((p) => p.pid), mode });
      showResult(label, list, r);
    } catch (e) {
      $('#modal').close();
      toast(e.message, true);
    }
    refresh();
  };
}

function showResult(label, list, r) {
  const left = r.remaining.map((x) => list.find((p) => p.pid === x.pid)).filter(Boolean);
  $('#modalBody').innerHTML = `
    <h3>${r.closed ? 'Done' : 'Nothing closed'}</h3>
    <p>${r.closed} of ${r.closed + r.remaining.length} process${r.closed + r.remaining.length === 1 ? '' : 'es'} closed${r.freedMB ? `, about ${fmt(r.freedMB)} freed` : ''}.</p>
    ${r.skipped.length ? `<div class="muted">Skipped ${r.skipped.length}: ${esc(r.skipped.slice(0, 4).map((s) => `${s.name || s.pid} (${s.reason})`).join(', '))}</div>` : ''}
    ${left.length ? `<div class="warn-box">${left.length} still running. A normal close was ignored (the app may be waiting on a save prompt, or minimised to the tray).</div>` : ''}
    <div class="modal-actions">${left.length ? '<button class="btn danger" id="mForce">Force close remaining</button>' : ''}<button class="btn ghost" id="mDone">Close</button></div>`;
  $('#mDone').onclick = () => $('#modal').close();
  if (left.length) $('#mForce').onclick = () => { $('#modal').close(); openClose(label, left, 'force'); };
}

async function busyButton(sel, label, fn) {
  const bs = [...document.querySelectorAll(sel)];
  const texts = bs.map((b) => b.textContent);
  bs.forEach((b) => { b.disabled = true; b.textContent = label; });
  try { await fn(); } catch (e) { toast(e.message, true); }
  bs.forEach((b, i) => { b.disabled = false; b.textContent = texts[i]; });
  refresh();
}

const trim = () => busyButton('#trimBtn', 'Trimming', async () => {
  const r = await api('/api/trim', {});
  toast(`Trimmed ${r.trimmed} apps. Free memory changed by ${r.freedMB >= 0 ? '+' : ''}${fmt(r.freedMB)}. Apps pull pages back as they need them.`);
});

const quickClean = () => busyButton('.clean-btn', 'Cleaning', async () => {
  const r = await api('/api/quickclean', {});
  const freed = `${r.freedMB >= 0 ? 'Freed' : 'Changed by'} ${fmt(Math.abs(r.freedMB))}.`;
  toast(r.closedApps.length
    ? `${freed} Closed ${r.closedApps.join(', ')} and trimmed ${r.trimmed} apps.`
    : `${freed} Nothing idle to close, so ${r.trimmed} apps were trimmed.`);
  if (r.skippedApps.length) toast(`Left open because it has a window: ${r.skippedApps.join(', ')}. Use What to close if you want it gone.`);
});

// ---------- Graphics ----------

async function loadGpuPage() {
  let r;
  try { r = await api('/api/gpu'); } catch (e) { return toast(e.message, true); }
  const g = r.gpu;
  const dgpu = r.adapters.find((a) => !a.integrated);
  const load = g?.loadPct ?? (dgpu ? Math.round(dgpu.loadPct) : null);
  if (g || dgpu) S.gpuHist.push([Date.now(), load ?? 0, g?.tempC ?? null]);
  S.gpuHot = (g?.slowdownC ?? 97) - 5;

  $('#gpuBigWrap').className = 'pct lvl-' + (g ? g.status.level : 'good');
  $('#gpuBig').textContent = g?.tempC ?? (load != null ? load : '–');
  $('#gpuUnit').textContent = g?.tempC != null ? '°C' : load != null ? '%' : '';
  $('#gpuStatus').textContent = g ? g.status.text : dgpu ? `Working at ${load}%.` : 'No graphics card found.';
  const note = $('#gpuNote');
  note.hidden = !!g;
  if (!g) note.textContent = `NVIDIA's driver did not answer, so temperature, clocks and power are missing. Load comes from Windows instead.${r.error ? ` (${r.error})` : ''}`;

  const of = (v, max, unit, k = 1) => (v != null ? `${(v / k).toFixed(k > 1 ? 1 : 0)}${max ? ` of ${(max / k).toFixed(k > 1 ? 1 : 0)}` : ''} ${unit}` : '–');
  $('#gpuFacts').innerHTML = [
    [load != null ? `${load}%` : '–', 'Load'],
    [of(g?.clockMhz, null, 'GHz', 1000), g?.maxClockMhz ? `Clock, up to ${ghz(g.maxClockMhz)} GHz` : 'Clock'],
    [g?.powerW != null ? `${g.powerW.toFixed(1)} W` : '–', g?.powerLimitW ? `Power draw, limit ${Math.round(g.powerLimitW)} W` : 'Power draw'],
    [g?.vramUsedMb != null ? fmt(g.vramUsedMb) : '–', g?.vramTotalMb ? `of ${fmt(g.vramTotalMb)} video memory` : 'Video memory'],
  ].map(([v, l]) => `<div><b>${esc(v)}</b><span>${esc(l)}</span></div>`).join('');

  $('#gpuName').textContent = g?.name || dgpu?.name || 'Engines';
  const engines = (dgpu?.engines || []).map((e) => [e.kind, e.pct]);
  if (g?.memLoadPct != null) engines.push(['Memory bus', g.memLoadPct]);
  const wrap = $('#gpuEngines');
  if (wrap.children.length !== engines.length) wrap.innerHTML = engines.map(() => '<div class="core"><i class="fill"></i><b></b><span></span></div>').join('');
  engines.forEach(([kind, pct], i) => {
    const el = wrap.children[i];
    el.classList.toggle('busy', pct >= 60);
    el.querySelector('.fill').style.height = Math.min(100, pct) + '%';
    el.querySelector('b').innerHTML = `${Math.round(pct)}<small>%</small>`;
    el.querySelector('span').textContent = kind;
  });
  if (!engines.length) wrap.innerHTML = `<div class="empty">${esc(r.countersError || 'Windows is not reporting engine use for this graphics card.')}</div>`;
  drawGpuChart();

  const groups = S.state?.groups || [];
  $('#gpuApps').innerHTML = r.apps.length ? r.apps.map((a) => {
    const grp = groups.find((x) => x.procs.some((p) => p.pid === a.pid));
    const act = !grp ? '' : grp.protected ? `<span class="lock">Protected, ${esc(grp.reason)}</span>` : `<button class="btn small ghost" data-close-app="${esc(grp.app)}">Close</button>`;
    return `<div class="rec"><div class="save">${a.pct.toFixed(a.pct < 10 ? 1 : 0)}%</div><div><div class="t">${esc(grp?.app || a.app)}</div><p class="w">${esc(a.adapter)}, ${esc(a.engine === '3D' ? a.engine : a.engine.toLowerCase())}</p></div><div class="act">${act}</div></div>`;
  }).join('') : '<div class="empty">No app is using graphics right now.</div>';

  $('#adapters').innerHTML = r.adapters.map((a) => {
    const mem = a.integrated ? `${fmt(a.sharedMb + a.dedicatedMb)} of main memory` : `${fmt(a.dedicatedMb)} of ${fmt(a.dedicatedTotalMb)} video memory`;
    const rest = !a.integrated && g?.pstate >= 8 && a.loadPct < 1 ? ', resting' : '';
    return `<div class="${a.loadPct >= 1 ? 'on' : ''}"><b>${Math.round(a.loadPct)}<small>%</small></b><span class="t">${esc(a.short)}${a.integrated ? ', built in' : ''}${rest}</span><span>${esc(mem)}</span></div>`;
  }).join('') || `<div class="none"><b>Graphics counters unavailable</b><span>${esc(r.countersError || '')}</span></div>`;

  const pstate = g?.pstate != null ? `P${g.pstate}` : '–';
  $('#gpuDetails').innerHTML = g ? [
    [pstate, g.pstate == null ? 'Power state' : g.pstate >= 8 ? 'Power state: resting' : g.pstate === 0 ? 'Power state: full speed' : 'Power state: partly awake'],
    [g.limits.length ? g.limits.join(', ') : 'Nothing', 'Holding the clock back'],
    [of(g.memClockMhz, g.maxMemClockMhz, 'GHz', 1000), 'Memory clock'],
    [g.slowdownC ? `${g.slowdownC} °C` : '–', 'Slows itself down from'],
    [`${g.encoderPct ?? 0}% / ${g.decoderPct ?? 0}%`, 'Video encoder / decoder'],
    [g.pcie || '–', 'Link to the processor (drops when idle)'],
    [g.driver || '–', 'NVIDIA driver'],
    [r.adapters.length ? String(r.adapters.length) : '–', 'Graphics chips'],
  ].map(([v, l]) => `<div><b>${esc(v)}</b><span>${esc(l)}</span></div>`).join('') : '';
}

// ---------- Settings ----------

const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const hourLabel = (h) => new Date(2026, 0, 4, h).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
const sw = (attr, on, label, disabled) => `<label class="switch"><input type="checkbox" ${attr} ${on ? 'checked' : ''} ${disabled ? 'disabled' : ''} aria-label="${esc(label)}"><span></span></label>`;
const num = (path, v, unit, min, max) => `<span class="inp"><input type="number" data-set="${path}" value="${v}" min="${min}" max="${max}"><i>${unit}</i></span>`;

async function loadSettings() {
  try { S.cfg = await api('/api/settings'); } catch (e) { return toast(e.message, true); }
  renderSettings();
}

function weeklyLine() {
  const w = S.cfg.settings.weekly;
  const last = w.lastRun ? `Last ran ${new Date(w.lastRun).toLocaleString([], { weekday: 'long', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' })}.` : 'Has not run yet.';
  if (!w.enabled) return `Off. ${last}`;
  const next = S.cfg.nextWeekly ? new Date(S.cfg.nextWeekly).toLocaleString([], { weekday: 'long', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }) : '';
  return `Next: ${next}. ${last} If the laptop is off then, it runs the next time Shimmy is open.`;
}

function renderSettings() {
  const { settings: st, defaultRoots, categories, startWithWindows } = S.cfg;
  const a = st.alerts, c = st.autoClean, w = st.weekly, f = st.finder;
  const block = (title, text, head, body) => `<div class="set-block"><div class="set-head"><div><h3>${title}</h3><p>${text}</p></div>${head || ''}</div>${body || ''}</div>`;
  const field = (label, input) => `<label class="field"><span>${label}</span>${input}</label>`;
  $('#settings').innerHTML = [
    block('Alerts', 'A Windows notification when something needs you. Each one repeats at most every half hour while it lasts.', sw('data-set="alerts.enabled"', a.enabled, 'Alerts'),
      `<div class="set-grid ${a.enabled ? '' : 'dim'}">
        ${field('Memory in use, for a minute', num('alerts.ramPct', a.ramPct, '%', 50, 99))}
        ${field('Processor temperature, for 30 seconds', num('alerts.cpuTempC', a.cpuTempC, '°C', 60, 105))}
        ${field('Free space on C: below', num('alerts.diskFreeGb', a.diskFreeGb, 'GB', 1, 200))}
        ${field('Battery, when unplugged, at or below', num('alerts.batteryPct', a.batteryPct, '%', 5, 50))}
      </div>`),
    block('Automatic memory clean', 'When memory stays full, Shimmy cleans it for you and tells you what it did. At most once every 15 minutes.', sw('data-set="autoClean.enabled"', c.enabled, 'Automatic memory clean'),
      `<div class="set-grid ${c.enabled ? '' : 'dim'}">
        ${field('When memory stays above', num('autoClean.atPct', c.atPct, '%', 50, 98))}
        ${field('For', num('autoClean.forMin', c.forMin, 'minutes', 1, 60))}
      </div>
      <div class="limits two ${c.enabled ? '' : 'dim'}">
        <button data-level="trim" class="${c.level === 'trim' ? 'on' : ''}"><b>Trim</b><span>Asks every app to give back idle memory. Closes nothing.</span></button>
        <button data-level="clean" class="${c.level === 'clean' ? 'on' : ''}"><b>Clean now</b><span>Also closes idle background apps marked safe. Anything with a window stays open.</span></button>
      </div>`),
    block('Weekly clean', 'Clears temporary files and caches once a week, the same as the Clean page. Browser caches are skipped while that browser is open.', sw('data-set="weekly.enabled"', w.enabled, 'Weekly clean'),
      `<div class="set-grid ${w.enabled ? '' : 'dim'}">
        ${field('Day', `<select data-set="weekly.day">${DAYS.map((d, i) => `<option value="${i}" ${w.day === i ? 'selected' : ''}>${d}</option>`).join('')}</select>`)}
        ${field('Time', `<select data-set="weekly.hour">${Array.from({ length: 24 }, (_, h) => `<option value="${h}" ${w.hour === h ? 'selected' : ''}>${hourLabel(h)}</option>`).join('')}</select>`)}
      </div>
      <div class="ticks ${w.enabled ? '' : 'dim'}">${categories.map((k) => `<label><input type="checkbox" class="tick" data-wcat="${k.id}" ${w.cats.includes(k.id) ? 'checked' : ''}>${esc(k.title)}</label>`).join('')}</div>
      <p class="set-note" id="weeklyLine">${esc(weeklyLine())}</p>`),
    block('Heat protection', `When the processor stays at 90 °C or more for 30 seconds, Shimmy turns CPU boost off until it cools to 75 °C for 2 minutes. Quiet: it shows on the Processor page and in the Log.`, sw('data-set="heat.enabled"', st.heat.enabled, 'Heat protection')),
    block('Start with Windows', 'Starts Shimmy in the tray when you sign in, with administrator rights and no prompt, through a Task Scheduler task. Works on battery too.', sw('data-autostart', startWithWindows, 'Start with Windows')),
    block('About', 'Shimmy Optimizer, founded and built by Shimrith Raj. Open source under the MIT licence; the licences of the parts it includes are in the legal folder where Shimmy is installed.'),
    block('Big and duplicate files', 'Where the finder on the Clean page looks, and what it never offers. Code projects, protected work folders, hidden files and your Keep list are always left out.', '',
      `<div class="set-grid">${field('Count a file as large from', num('finder.largeMb', f.largeMb, 'MB', 50, 100000))}${field('Old caches: files untouched for', num('caches.ageDays', st.caches.ageDays, 'days', 1, 365))}</div>
      <h4>Folders</h4>
      <div class="rows">
        ${defaultRoots.map((r) => `<div class="row"><span>${esc(r)}</span>${sw(`data-root="${esc(r)}"`, !f.skipRoots.includes(r), 'Search ' + r)}</div>`).join('')}
        ${f.extraRoots.map((r, i) => `<div class="row"><span>${esc(r)}</span><button class="btn small ghost" data-rm-root="${i}">Remove</button></div>`).join('')}
      </div>
      <div class="add"><input type="text" id="newRoot" placeholder="Another folder, for example D:\\Projects\\Media" aria-label="Folder to add"><button class="btn small ghost" id="addRoot">Add folder</button></div>
      <h4>Never offer files whose path contains</h4>
      <div class="rows">${f.exclusions.map((x, i) => `<div class="row"><span>${esc(x)}</span><button class="btn small ghost" data-rm-excl="${i}">Remove</button></div>`).join('') || '<div class="row"><span class="muted">Nothing added yet.</span></div>'}</div>
      <div class="add"><input type="text" id="newExcl" placeholder="A word or folder name, for example Taxes" aria-label="Exclusion to add"><button class="btn small ghost" id="addExcl">Add</button></div>`),
  ].join('');
}

let saveTimer = null;
function saveSettings(rerender) {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      S.cfg = await api('/api/settings/save', { settings: S.cfg.settings });
      if (rerender) renderSettings();
      else $('#weeklyLine').textContent = weeklyLine();
    } catch (e) {
      toast(e.message, true);
      loadSettings();
    }
  }, 300);
}

function setPath(path, v) {
  const keys = path.split('.');
  const last = keys.pop();
  keys.reduce((o, k) => o[k], S.cfg.settings)[last] = v;
}

$('#settings').addEventListener('change', async (e) => {
  const t = e.target;
  const st = S.cfg.settings;
  if (t.dataset.set) {
    setPath(t.dataset.set, t.type === 'checkbox' ? t.checked : Number(t.value));
    return saveSettings(t.type === 'checkbox');
  }
  if (t.dataset.wcat) {
    st.weekly.cats = st.weekly.cats.filter((c) => c !== t.dataset.wcat);
    if (t.checked) st.weekly.cats.push(t.dataset.wcat);
    return saveSettings();
  }
  if (t.dataset.root) {
    st.finder.skipRoots = st.finder.skipRoots.filter((r) => r !== t.dataset.root);
    if (!t.checked) st.finder.skipRoots.push(t.dataset.root);
    return saveSettings();
  }
  if ('autostart' in t.dataset) {
    t.disabled = true;
    try {
      const r = await api('/api/autostart', { on: t.checked });
      t.checked = r.on;
      toast(r.on ? 'Shimmy will start in the tray when you sign in.' : 'Shimmy will no longer start with Windows.');
    } catch (err) { toast(err.message, true); t.checked = !t.checked; }
    t.disabled = false;
  }
});

$('#settings').addEventListener('click', (e) => {
  const b = e.target.closest('button');
  if (!b) return;
  const f = S.cfg.settings.finder;
  if (b.dataset.level) { S.cfg.settings.autoClean.level = b.dataset.level; return saveSettings(true); }
  if (b.dataset.rmRoot != null) { f.extraRoots.splice(Number(b.dataset.rmRoot), 1); return saveSettings(true); }
  if (b.dataset.rmExcl != null) { f.exclusions.splice(Number(b.dataset.rmExcl), 1); return saveSettings(true); }
  const add = (input, list) => {
    const v = $(input).value.trim();
    if (v && !list.includes(v)) { list.push(v); saveSettings(true); }
  };
  if (b.id === 'addRoot') add('#newRoot', f.extraRoots);
  if (b.id === 'addExcl') add('#newExcl', f.exclusions);
});

$('#settings').addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && (e.target.id === 'newRoot' || e.target.id === 'newExcl')) $(e.target.id === 'newRoot' ? '#addRoot' : '#addExcl').click();
});

// ---------- Startup & log ----------

const HINTS = [[/spotify/i, 'Music app: open it when you want it'], [/roblox/i, 'Game launcher'], [/mega/i, 'Cloud sync: keep if you rely on it'], [/screenrec/i, 'Screen recorder'], [/acrobat/i, 'Adobe sync helper: rarely needed'], [/edge/i, 'Edge pre-launch: pure background load'], [/securityhealth/i, 'Windows security icon: leave on'], [/seagull/i, 'Printer drivers: leave on if you use that printer']];

async function loadStartup() {
  try {
    S.startup = (await api('/api/startup')).items;
    $('#startupTable tbody').innerHTML = S.startup.map((s, i) => {
      const hint = (HINTS.find((h) => h[0].test(s.name + s.command)) || [])[1] || '';
      return `<tr><td><b>${esc(s.name)}</b></td><td class="cmd-cell" title="${esc(s.command)}">${esc(s.command)}</td><td>${esc(s.source)}${s.scope === 'Machine' ? ', all users' : ''}</td><td class="muted">${esc(hint)}</td><td class="num"><label class="switch"><input type="checkbox" data-su="${i}" ${s.enabled ? 'checked' : ''} ${s.canToggle ? '' : 'disabled'} aria-label="Run ${esc(s.name)} at startup"><span title="${s.canToggle ? '' : 'Needs administrator rights'}"></span></label></td></tr>`;
    }).join('') || '<tr><td colspan="5" class="muted">No startup entries found.</td></tr>';
    const heavy = S.startup.filter((s) => s.enabled && HINTS.slice(0, 6).some((h) => h[0].test(s.name + s.command)));
    $('#startupHint').innerHTML = heavy.length ? `<div class="rec"><div class="save none">At boot</div><div><div class="t">${heavy.length} apps load every time you sign in<span class="risk tip">Tip</span></div><p class="w">${esc(heavy.map((s) => s.name).join(', '))}. Turning off the ones you don't need saves RAM from the moment Windows starts.</p></div><div class="act"><button class="btn small ghost" data-goto="startup">Open Startup</button></div></div>` : '';
  } catch (e) { toast(e.message, true); }
}

async function loadLog() {
  try {
    const rows = (await api('/api/log')).rows;
    $('#logTable tbody').innerHTML = rows.map((r) => `<tr><td class="num">${esc(new Date(r.time).toLocaleString())}</td><td>${esc(r.action)}</td><td>${esc(r.target)}</td><td class="muted">${esc(r.detail)}</td></tr>`).join('') || '<tr><td colspan="4" class="muted">Nothing logged yet.</td></tr>';
  } catch (e) { toast(e.message, true); }
}

// ---------- Wiring ----------

const revealer = 'IntersectionObserver' in window
  ? new IntersectionObserver((es) => es.forEach((e) => { if (e.isIntersecting) { e.target.classList.add('in'); revealer.unobserve(e.target); } }), { threshold: 0.1 })
  : null;
function watchReveals() {
  document.querySelectorAll('section:not([hidden]) .reveal:not(.in)').forEach((el) => (revealer ? revealer.observe(el) : el.classList.add('in')));
}

function setTab(t) {
  S.tab = t;
  document.querySelectorAll('#tabs button').forEach((b) => b.classList.toggle('on', b.dataset.tab === t));
  document.querySelectorAll('main > section').forEach((s) => (s.hidden = s.id !== 'tab-' + t));
  window.scrollTo(0, 0);
  if (t === 'startup') loadStartup();
  if (t === 'log') loadLog();
  if (t === 'apps') {
    const wrap = $('#tab-apps .table-wrap');
    wrap.classList.add('grow-in');
    clearTimeout(S.growTimer);
    S.growTimer = setTimeout(() => wrap.classList.remove('grow-in'), 900);
    if (S.state) renderApps();
  }
  if (t === 'memory') requestAnimationFrame(drawChart);
  if (t === 'cpu') { if (S.state) renderCpu(); loadGpu(); }
  if (t === 'storage') loadStorage();
  if (t === 'clean') loadClean();
  if (t === 'battery') loadBattery();
  if (t === 'gpu') { requestAnimationFrame(drawGpuChart); loadGpuPage(); }
  if (t === 'settings') loadSettings();
  watchReveals();
}

document.addEventListener('click', async (e) => {
  const del = e.target.closest('[data-del]');
  if (del) return previewDelete(del.dataset.del);
  const uc = e.target.closest('[data-ucat]');
  if (uc && S.storage) {
    const id = uc.dataset.ucat, prof = S.storage.profile;
    if (id === 'apps') return api('/api/apps-settings').catch((err) => toast(err.message, true));
    if (id === 'yours' || id === 'appdata') { loadFolder(id === 'yours' ? prof : prof + '\\AppData'); return $('#folders').scrollIntoView({ behavior: 'smooth', block: 'center' }); }
    if (id !== 'free') return $('#sysFiles').scrollIntoView({ behavior: 'smooth', block: 'center' });
    return;
  }
  if (e.target.id === 'binEmpty') return confirmEmptyBin();
  if (e.target.id === 'appsOpen') return api('/api/apps-settings').catch((err) => toast(err.message, true));
  const open = e.target.closest('[data-open]');
  if (open) {
    e.preventDefault();
    return api('/api/open', { path: open.dataset.open }).catch((err) => toast(err.message, true));
  }
  const folder = e.target.closest('[data-folder]');
  if (folder) return loadFolder(folder.dataset.folder);
  const fanBtn = e.target.closest('[data-fan]');
  if (fanBtn) {
    fanBtn.disabled = true;
    try {
      const r = await api('/api/fan', { mode: fanBtn.dataset.fan });
      S.state.fanMode = r.mode;
      toast(`Fan mode: ${FAN_MODES.find((m) => m[0] === r.mode)?.[1] || r.mode}.`);
    } catch (err) { toast(err.message, true); }
    return renderFan();
  }
  const lim = e.target.closest('[data-limit]');
  if (lim) return setLimit(Number(lim.dataset.limit));
  const dism = e.target.closest('[data-dism]');
  if (dism) {
    await api('/api/dism', { cleanup: dism.dataset.dism === 'cleanup' });
    return loadClean();
  }
  const t = e.target.closest('button, a[data-tab], tr.group, .map i[data-c]');
  if (!t) return;
  if (t.dataset.tab) { e.preventDefault(); return setTab(t.dataset.tab); }
  if (t.dataset.goto) return setTab(t.dataset.goto);
  if (t.dataset.gr) {
    S.gpuRange = Number(t.dataset.gr);
    document.querySelectorAll('#gpuRange button').forEach((b) => b.classList.toggle('on', b === t));
    return drawGpuChart();
  }
  if (t.dataset.cr) {
    S.cpuRange = Number(t.dataset.cr);
    document.querySelectorAll('#cpuRange button').forEach((b) => b.classList.toggle('on', b === t));
    return drawCpuChart();
  }
  if (t.dataset.r) {
    S.range = Number(t.dataset.r);
    document.querySelectorAll('#range button').forEach((b) => b.classList.toggle('on', b === t));
    return drawChart();
  }
  if (t.dataset.seg != null || t.dataset.c != null) {
    const s = S.segs[Number(t.dataset.seg ?? t.dataset.c)];
    if (!s || s.c === 0) return;
    S.filter = s.app; $('#filter').value = s.app;
    S.open.add(s.app);
    return setTab('apps');
  }
  if ('trim' in t.dataset) return trim();
  if (t.dataset.rec != null) {
    const r = S.state.recs[Number(t.dataset.rec)];
    const procs = S.state.groups.flatMap((g) => g.procs);
    return openClose(r.title, r.pids.map((pid) => procs.find((p) => p.pid === pid)).filter(Boolean), 'auto', r.risk === 'review' ? r.why : '');
  }
  if (t.dataset.closeApp) {
    e.stopPropagation();
    const g = S.state.groups.find((x) => x.app === t.dataset.closeApp);
    return openClose(g.app, g.procs.filter((p) => !p.protected));
  }
  if (t.dataset.closePid) {
    const g = S.state.groups.find((x) => x.app === t.dataset.app);
    return openClose(g.app + ' process', g.procs.filter((p) => p.pid === Number(t.dataset.closePid)));
  }
  if (t.classList.contains('group')) {
    const a = t.dataset.app;
    S.open.has(a) ? S.open.delete(a) : S.open.add(a);
    renderApps();
  }
});

document.addEventListener('change', async (e) => {
  if (e.target.dataset?.cat) {
    e.target.checked ? S.cleanSel.add(e.target.dataset.cat) : S.cleanSel.delete(e.target.dataset.cat);
    return loadClean();
  }
  if (e.target.dataset?.cache) {
    e.target.checked ? S.cacheSel.add(e.target.dataset.cache) : S.cacheSel.delete(e.target.dataset.cache);
    return renderCaches(S.storage);
  }
  if (e.target.dataset && 'hiber' in e.target.dataset) return confirmHibernation(e.target);
  if (e.target.dataset?.file) {
    e.target.checked ? S.finderSel.add(e.target.dataset.file) : S.finderSel.delete(e.target.dataset.file);
    return updateFinderBar();
  }
  if (e.target.dataset && 'heat' in e.target.dataset) {
    e.target.disabled = true;
    try {
      await api('/api/heat', { on: e.target.checked });
      S.state.heat.enabled = e.target.checked;
      if (!e.target.checked) S.state.heat.boostOff = false;
      toast(e.target.checked ? 'Heat protection is on.' : 'Heat protection is off. Boost is on.');
    } catch (err) { toast(err.message, true); }
    return renderFan();
  }
  if (e.target.dataset && 'hold' in e.target.dataset) {
    e.target.disabled = true;
    try {
      const r = await api('/api/plugged', { on: e.target.checked });
      toast(r.on ? `Running from the charger. The battery stays at ${r.limit}% while plugged in.` : `Charging normally again, up to ${r.limit}%.`);
    } catch (err) { toast(err.message, true); }
    return loadBattery();
  }
  const i = e.target.dataset?.su;
  if (i == null) return;
  const s = S.startup[Number(i)];
  e.target.disabled = true;
  try {
    await api('/api/startup/toggle', { name: s.name, sub: s.sub, scope: s.scope, enable: e.target.checked });
    toast(`${s.name} will ${e.target.checked ? 'now' : 'no longer'} start with Windows.`);
  } catch (err) { toast(err.message, true); e.target.checked = !e.target.checked; }
  e.target.disabled = false;
});

const hoverSeg = (e) => {
  const el = e.target.closest('.map i, .legend button');
  if (el) focusSeg(el.dataset.c ?? el.dataset.seg ?? null);
};
$('#map').addEventListener('mouseover', hoverSeg);
$('#legend').addEventListener('mouseover', hoverSeg);
$('#legend').addEventListener('focusin', hoverSeg);
$('.map-block').addEventListener('mouseleave', () => focusSeg(null));
$('#filter').addEventListener('input', (e) => { S.filter = e.target.value; if (S.state) renderApps(); });
$('#trimBtn').addEventListener('click', trim);
$('#usageBtn').addEventListener('click', async () => { S.folderLoaded = false; await api('/api/usage/scan'); loadStorage(); });
$('#cleanRun').addEventListener('click', async () => { S.cleanWasRunning = true; await api('/api/clean/run', { ids: [...S.cleanSel] }); loadClean(); });
$('#cleanRescan').addEventListener('click', async () => { await api('/api/clean/scan'); loadClean(); });
$('#finderBtn').addEventListener('click', async () => { S.finderSel.clear(); S.finderAt = 0; updateFinderBar(); await api('/api/finder'); loadClean(); });
$('#finderDelete').addEventListener('click', () => confirmDelete([...S.finderSel]));
$('#cacheBtn').addEventListener('click', async () => { await api('/api/caches/scan'); loadStorage(); });
$('#cacheGo').addEventListener('click', confirmCaches);
document.querySelectorAll('.clean-btn').forEach((b) => b.addEventListener('click', quickClean));
$('#chart').addEventListener('mousemove', (e) => { S.hoverX = e.offsetX; drawChart(); });
$('#chart').addEventListener('mouseleave', () => { S.hoverX = null; drawChart(); });
$('#cpuChart').addEventListener('mousemove', (e) => { S.cpuHoverX = e.offsetX; drawCpuChart(); });
$('#cpuChart').addEventListener('mouseleave', () => { S.cpuHoverX = null; drawCpuChart(); });
$('#wearChart').addEventListener('mousemove', (e) => { S.wearHoverX = e.offsetX; drawWearChart(); });
$('#wearChart').addEventListener('mouseleave', () => { S.wearHoverX = null; drawWearChart(); });
$('#powerChart').addEventListener('mousemove', (e) => { S.powerHoverX = e.offsetX; drawPowerChart(); });
$('#powerChart').addEventListener('mouseleave', () => { S.powerHoverX = null; drawPowerChart(); });
$('#gpuChart').addEventListener('mousemove', (e) => { S.gpuHoverX = e.offsetX; drawGpuChart(); });
$('#gpuChart').addEventListener('mouseleave', () => { S.gpuHoverX = null; drawGpuChart(); });
window.addEventListener('resize', () => { drawChart(); drawCpuChart(); drawGpuChart(); drawWearChart(); drawPowerChart(); });

let busy = false;
async function refresh() {
  if (busy) return;
  busy = true;
  try {
    S.state = await api('/api/state');
    S.history.push([S.state.at, S.state.mem.usedMB]);
    S.cpuHist.push([S.state.at, S.state.cpu.loadPct, S.state.cpu.tempC]);
    $('#dot').className = 'ok';
    $('#updated').textContent = 'Live, ' + new Date(S.state.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    renderHome(); renderHero(); renderMap(); renderRecs(); renderApps(); drawChart();
    if (S.tab === 'cpu') { renderCpu(); loadGpu(); }
    if (S.tab === 'storage') loadStorage();
    if (S.tab === 'clean') loadClean();
    if (S.tab === 'battery') loadBattery();
    if (S.tab === 'gpu') loadGpuPage();
    if (S.focus != null) focusSeg(S.focus);
  } catch (e) {
    $('#dot').className = 'err';
    $('#updated').textContent = 'Disconnected: ' + e.message;
  }
  busy = false;
}

(async () => {
  watchReveals();
  try {
    const h = await api('/api/history');
    S.history = h.points;
    S.cpuHist = h.cpu;
  } catch { /* charts fill in as samples arrive */ }
  await refresh();
  loadStartup();
  setInterval(() => { if (!document.hidden && !['log', 'settings'].includes(S.tab) && !$('#modal').open) refresh(); }, 5000);
  // A clean started from the tray asks the window to refresh.
  window.__TAURI__.event.listen('refresh', () => refresh());
})();
