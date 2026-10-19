// CPU consommé par la mascotte, mesuré sur l'arbre de processus (Linux, /proc).
//
//   Chromium headless :  node scripts/cpu.mjs chromium [--states idle,think] [--seconds 30]
//   App Tauri (WebKitGTK) : node scripts/cpu.mjs tauri --bin ../target/debug/kobi [--seconds 30]
//
// Le pourcentage est donné pour un cœur (100 % = un cœur occupé) et pour la
// machine entière (divisé par le nombre de cœurs, comme le gestionnaire des
// tâches de Windows). Un échauffement de 5 s précède chaque mesure.

import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { cpus } from 'node:os';
import { chromium } from '@playwright/test';

const TICK = 100; // CLK_TCK sous Linux
const CORES = cpus().length;

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
}
const mode = process.argv[2] ?? 'chromium';
const seconds = Number(arg('seconds', '30'));
const warmup = Number(arg('warmup', '5'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** { pid: { ppid, comm, ticks } } pour tous les processus. */
function snapshot() {
  const out = new Map();
  for (const d of readdirSync('/proc')) {
    if (!/^\d+$/.test(d)) continue;
    try {
      const stat = readFileSync(`/proc/${d}/stat`, 'utf8');
      const close = stat.lastIndexOf(')');
      const comm = stat.slice(stat.indexOf('(') + 1, close);
      const f = stat.slice(close + 2).split(' ');
      out.set(Number(d), { ppid: Number(f[1]), comm, ticks: Number(f[11]) + Number(f[12]) });
    } catch {
      // processus terminé entre-temps
    }
  }
  return out;
}

function descendants(snap, root) {
  const set = new Set([root]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const [pid, p] of snap) if (!set.has(pid) && set.has(p.ppid)) (set.add(pid), (grew = true));
  }
  return set;
}

/** CPU par commande de processus sur `seconds`, pour l'arbre de `root`. */
async function measure(root, filter = () => true) {
  const a = snapshot();
  const t0 = performance.now();
  await sleep(seconds * 1000);
  const b = snapshot();
  const wall = (performance.now() - t0) / 1000;
  const byComm = new Map();
  for (const pid of descendants(b, root)) {
    const p = b.get(pid);
    if (!p || !filter(p)) continue;
    const before = a.get(pid)?.comm === p.comm ? a.get(pid).ticks : 0;
    byComm.set(p.comm, (byComm.get(p.comm) ?? 0) + (p.ticks - before) / TICK);
  }
  const total = [...byComm.values()].reduce((x, y) => x + y, 0);
  return { wall, total, byComm };
}

function row(label, m) {
  const core = (100 * m.total) / m.wall;
  const detail = [...m.byComm].filter(([, s]) => s > 0).map(([c, s]) => `${c} ${((100 * s) / m.wall).toFixed(1)} %`).join(', ');
  return `| ${label} | ${m.wall.toFixed(0)} s | ${core.toFixed(2)} % | ${(core / CORES).toFixed(2)} % | ${detail || '—'} |`;
}

const header = [
  `| Mesure | Durée | CPU (1 cœur = 100 %) | CPU machine (${CORES} cœurs) | Détail par processus |`,
  '| -- | -- | -- | -- | -- |',
];

if (mode === 'chromium') {
  const states = arg('states', 'idle').split(',');
  const PORT = 4176;
  const server = spawn('node', ['node_modules/vite/bin/vite.js', 'preview', '--port', String(PORT), '--strictPort', '--host', '127.0.0.1'], {
    stdio: 'ignore',
    detached: true,
  });
  try {
    for (let i = 0; i < 150; i++) {
      if (await fetch(`http://127.0.0.1:${PORT}/`).then(() => true, () => false)) break;
      await sleep(200);
    }
    const browser = await chromium.launch();
    // Le navigateur est un descendant de ce script ; le serveur Vite (node) est exclu.
    const pid = process.pid;
    const notNode = (p) => p.comm !== 'node';
    const page = await browser.newPage({ viewport: { width: 410, height: 280 } });
    console.log(header.join('\n'));
    for (const state of states) {
      await page.goto(`http://127.0.0.1:${PORT}/?state=${state}`);
      await page.waitForFunction(() => 'kobi' in window);
      await sleep(warmup * 1000);
      console.log(row(`Chromium headless, ${state}`, await measure(pid, notNode)));
    }
    // Fenêtre masquée : le navigateur coupe le rendu et la scène met tout en pause.
    await page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await sleep(warmup * 1000);
    console.log(row(`Chromium headless, ${states.at(-1)}, page masquée (pause)`, await measure(pid, notNode)));
    await browser.close();
  } finally {
    process.kill(-server.pid);
  }
} else if (mode === 'tauri') {
  const bin = arg('bin', '../target/debug/kobi');
  // Écran virtuel dédié ; on mesure l'app et ses processus WebKit, puis le serveur X à part.
  const display = `:${90 + Math.floor(Math.random() * 9)}`;
  const xvfb = spawn('Xvfb', [display, '-screen', '0', '1920x1080x24', '-nolisten', 'tcp'], { stdio: 'ignore', detached: true });
  await sleep(1500);
  // HOME de test : premier lancement marqué comme fait, sinon l'accueil
  // (KOB-144) s'ouvre dans une seconde fenêtre animée et la mesure compte les
  // deux (KOB-170).
  const home = process.env.KOBI_HOME ?? mkdtempSync(join(tmpdir(), 'kobi-cpu-'));
  mkdirSync(join(home, 'Kobi'), { recursive: true });
  writeFileSync(join(home, 'Kobi', 'premier-lancement.fait'), '');
  const app = spawn(bin, [], { stdio: 'ignore', detached: true, env: { ...process.env, DISPLAY: display, HOME: home } });
  try {
    await sleep((warmup + 5) * 1000);
    const both = snapshot();
    if (!both.has(app.pid)) throw new Error("l'app s'est arrêtée au démarrage");
    const a = snapshot();
    const t0 = performance.now();
    await sleep(seconds * 1000);
    const b = snapshot();
    const wall = (performance.now() - t0) / 1000;
    const tally = (root) => {
      const byComm = new Map();
      for (const pid of descendants(b, root)) {
        const p = b.get(pid);
        const before = a.get(pid)?.comm === p.comm ? a.get(pid).ticks : 0;
        byComm.set(p.comm, (byComm.get(p.comm) ?? 0) + (p.ticks - before) / TICK);
      }
      return { wall, total: [...byComm.values()].reduce((x, y) => x + y, 0), byComm };
    };
    console.log(header.join('\n'));
    console.log(row('App Tauri (WebKitGTK), kobi, veille', tally(app.pid)));
    console.log(row('Serveur X virtuel (Xvfb), affichage de la fenêtre', tally(xvfb.pid)));
  } finally {
    process.kill(-app.pid);
    process.kill(-xvfb.pid);
  }
}
