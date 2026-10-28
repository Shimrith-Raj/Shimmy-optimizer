'use strict';
// Captures a real process snapshot and runs RAM Optimizer v0.2's own classify/recommend code on it, so the Rust port
// can be tested for identical output. Usage: node tools/make-fixture.js
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');

const OLD = path.join(os.homedir(), 'Desktop', 'RAM Optimizer');
const OUT = path.join(__dirname, '..', 'app', 'src-tauri', 'tests', 'fixtures');

const raw = JSON.parse(execFileSync('powershell.exe',
  ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', path.join(OLD, 'scripts', 'processes.ps1')],
  { maxBuffer: 64 * 1024 * 1024, windowsHide: true }).toString());

// v0.2's classification block, evaluated with a fake "process" so no pid is treated as the dashboard itself.
const src = fs.readFileSync(path.join(OLD, 'server.js'), 'utf8');
const block = src.slice(src.indexOf('// ---------- Classification ----------'), src.indexOf('// ---------- Memory history ----------'));
const v02 = new Function('process', 'MB', `${block}; return { buildGroups, recommend };`)({ pid: -1, ppid: -1 }, 1024 * 1024);

const total = os.totalmem(), free = os.freemem();
const mb = (b) => Math.round(b / 1048576);
const mem = { totalMB: mb(total), freeMB: mb(free), usedMB: mb(total - free), pct: Math.round(((total - free) / total) * 100) };

const groups = v02.buildGroups(raw);
const recs = v02.recommend(raw, groups, mem);

const procs = raw.map((p) => ({
  pid: p.pid, ppid: p.ppid, name: p.name || '', ws: p.ws, cmd: p.cmd || '', path: p.path || '', title: p.title || '',
  started: p.started ? Math.floor(Date.parse(p.started) / 1000) : null,
}));
const expected = {
  groups: groups.map((g) => ({ app: g.app, mb: g.mb, count: g.count, protected: g.protected, reason: g.reason, pids: g.procs.map((p) => p.pid) })),
  recs: recs.map((r) => ({ id: r.id, title: r.title, app: r.app ?? null, risk: r.risk, action: r.action, pids: r.pids, savingsMB: r.savingsMB, why: r.why })),
};

fs.mkdirSync(OUT, { recursive: true });
fs.writeFileSync(path.join(OUT, 'snapshot.json'), JSON.stringify({ mem, procs }));
fs.writeFileSync(path.join(OUT, 'expected.json'), JSON.stringify(expected, null, 1));
console.log(`${procs.length} processes, ${groups.length} groups, ${recs.length} suggestions:`);
recs.forEach((r) => console.log(`  ${r.risk.padEnd(6)} ${r.action.padEnd(5)} ${r.title} (${r.savingsMB} MB)`));
