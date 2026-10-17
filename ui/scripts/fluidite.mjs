// Fluidité des changements d'état : enchaîne toutes les transitions entre les
// 8 états et mesure l'intervalle entre images (requestAnimationFrame) dans les
// 600 ms qui suivent chaque changement, plus les tâches longues (> 50 ms).
//
//   npm run build && node scripts/fluidite.mjs [--rounds 2]
//
// Affiche un tableau markdown. Une image « manquée » dure plus de 1,5 × 16,7 ms.

import { spawn } from 'node:child_process';
import { chromium } from '@playwright/test';
import { stopTree } from './stop.mjs';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'];
const PORT = 4175;

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
}
const rounds = Number(arg('rounds', '1'));

const server = spawn('node', ['node_modules/vite/bin/vite.js', 'preview', '--port', String(PORT), '--strictPort', '--host', '127.0.0.1'], { stdio: 'ignore', detached: true });
const url = `http://127.0.0.1:${PORT}/?backdrop=1`;
for (let i = 0; i < 150; i++) {
  try {
    if ((await fetch(url)).ok) break;
  } catch {
    await new Promise((r) => setTimeout(r, 200));
  }
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 820, height: 560 } });
await page.goto(url);
await page.waitForFunction(() => 'kobi' in window);
await page.evaluate(() => document.fonts.ready);

const pairs = [];
for (let r = 0; r < rounds; r++) for (const a of STATES) for (const b of STATES) if (a !== b) pairs.push([a, b]);

const result = await page.evaluate(async (pairs) => {
  const longTasks = [];
  new PerformanceObserver((l) => l.getEntries().forEach((e) => longTasks.push(e.duration))).observe({ type: 'longtask', buffered: false });
  const deltas = [];
  const frames = (ms) =>
    new Promise((resolve) => {
      const out = [];
      let last = performance.now();
      const end = last + ms;
      const tick = (t) => {
        out.push(t - last);
        last = t;
        if (t < end) requestAnimationFrame(tick);
        else resolve(out);
      };
      requestAnimationFrame(tick);
    });
  for (const [a, b] of pairs) {
    window.kobi.setState(a, window.kobi.demoData(a));
    await frames(400);
    const p = frames(600);
    window.kobi.setState(b, window.kobi.demoData(b));
    deltas.push(...(await p).slice(1));
  }
  return { deltas, longTasks };
}, pairs);

await browser.close();
stopTree(server);

const d = result.deltas.sort((x, y) => x - y);
const q = (p) => d[Math.min(d.length - 1, Math.floor(p * d.length))].toFixed(1);
const missed = d.filter((x) => x > 25).length;
console.log(`| Transitions | Images mesurées | Médiane (ms) | p95 (ms) | p99 (ms) | Max (ms) | Images > 25 ms | Tâches longues |`);
console.log(`| -- | -- | -- | -- | -- | -- | -- | -- |`);
console.log(
  `| ${pairs.length} | ${d.length} | ${q(0.5)} | ${q(0.95)} | ${q(0.99)} | ${d[d.length - 1].toFixed(1)} | ${missed} (${((100 * missed) / d.length).toFixed(2)} %) | ${result.longTasks.length} |`,
);
