// Captures d'un skin pour les PR et la CI : une image PNG, une vidéo WebM et
// un GIF par état, plus un « tour » qui enchaîne les 8 états (transitions).
//
//   npm run build
//   node scripts/captures.mjs --skin kobi --out captures/kobi [--seconds 3] [--states idle,listen] [--no-gif]
//
// Les GIF demandent ffmpeg dans le PATH. Le script lance lui-même `vite preview`.

import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, renameSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'];
const W = 820;
const H = 560;
const PORT = 4174;

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  return process.argv[i + 1];
}

const skin = arg('skin', 'kobi');
const out = resolve(arg('out', `captures/${skin}`));
const seconds = Number(arg('seconds', '3'));
const states = arg('states', STATES.join(',')).split(',');
const gif = !process.argv.includes('--no-gif');
const tour = !process.argv.includes('--no-tour');
const extra = arg('query', '');

async function waitForServer(url, ms = 30_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {
      // pas encore prêt
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`serveur injoignable : ${url}`);
}

function toGif(webm, target) {
  // Palette dédiée pour un GIF net ; 25 images/s, demi-taille.
  const r = spawnSync(
    'ffmpeg',
    [
      '-y', '-loglevel', 'error', '-i', webm,
      '-vf', 'fps=25,scale=410:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4',
      target,
    ],
    { stdio: 'inherit' },
  );
  if (r.status !== 0) throw new Error(`ffmpeg a échoué pour ${webm}`);
}

async function record(browser, name, url, script) {
  const tmp = join(out, `.video-${name}`);
  const context = await browser.newContext({
    viewport: { width: W, height: H },
    recordVideo: { dir: tmp, size: { width: W, height: H } },
  });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  await page.goto(url);
  await page.waitForFunction(() => 'kobi' in window);
  await page.evaluate(() => document.fonts.ready);
  await script(page);
  await context.close();
  const [file] = readdirSync(tmp);
  const webm = join(out, `${name}.webm`);
  renameSync(join(tmp, file), webm);
  rmSync(tmp, { recursive: true, force: true });
  if (gif) toGif(webm, join(out, `${name}.gif`));
  if (errors.length) throw new Error(`${name} : erreurs dans la console\n${errors.join('\n')}`);
}

mkdirSync(out, { recursive: true });
// Un ancien serveur sur le port servirait un autre build : on refuse.
if (await fetch(`http://127.0.0.1:${PORT}/`).then(() => true, () => false)) {
  throw new Error(`port ${PORT} déjà occupé : arrêter l'ancien serveur`);
}
const server = spawn('node', ['node_modules/vite/bin/vite.js', 'preview', '--port', String(PORT), '--strictPort', '--host', '127.0.0.1'], {
  stdio: 'ignore',
  detached: true,
});
const base = `http://127.0.0.1:${PORT}/?skin=${skin}&backdrop=1${extra ? `&${extra}` : ''}`;

try {
  await waitForServer(base);
  const browser = await chromium.launch();
  for (const state of states) {
    await record(browser, state, base, async (page) => {
      // La page démarre en veille : on enregistre le passage vers l'état,
      // avec sa transition, comme dans l'application.
      await page.waitForTimeout(400);
      await page.evaluate((s) => (window.kobi.play ? window.kobi.play(s) : window.kobi.setState(s, window.kobi.demoData(s))), state);
      await page.waitForTimeout(seconds * 1000);
      await page.screenshot({ path: join(out, `${state}.png`) });
    });
    console.log(`${skin} · ${state} : ok`);
  }
  if (tour) {
    await record(browser, 'tour', base, async (page) => {
      for (const state of STATES) {
        await page.evaluate((s) => (window.kobi.play ? window.kobi.play(s) : window.kobi.setState(s, window.kobi.demoData(s))), state);
        await page.waitForTimeout(2200);
      }
    });
    console.log(`${skin} · tour : ok`);
  }
  await browser.close();
} finally {
  process.kill(-server.pid);
}
