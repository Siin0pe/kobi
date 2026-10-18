// Vidéos des animations de Kobi (KOB-137) : chaque état, chaque transition,
// la veille vivante, le curseur, une réunion chargée, la lisibilité sur fond
// sombre et « réduire les animations ». Une vidéo MP4 (H.264) par séquence.
//
//   npm run build
//   node scripts/videos.mjs --out videos [--only etat-idle,reunion] [--webm] [--dist dist]
//
// Demande ffmpeg dans le PATH (conversion WebM → MP4). Lance `vite preview`.

import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readdirSync, renameSync, rmSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';
import { stopTree } from './stop.mjs';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'];
const W = 820;
const H = 560;
const PORT = 4176;

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
}
const out = resolve(arg('out', 'videos'));
const only = arg('only', '')?.split(',').filter(Boolean) ?? [];
const keepWebm = process.argv.includes('--webm');
const dist = arg('dist', 'dist');
const base = `http://127.0.0.1:${PORT}/`;
const wait = (page, ms) => page.waitForTimeout(ms);
const play = (page, s) => page.evaluate((s) => window.kobi.play(s), s);
const set = (page, s) => page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), s);

/** Séquences : nom, paramètres d'URL, scénario. */
const CLIPS = [
  ...STATES.map((s) => [
    `etat-${s}`,
    'backdrop=1',
    async (page) => {
      await wait(page, 700);
      await play(page, s);
      await wait(page, s === 'idle' ? 14_000 : 6000);
    },
  ]),
  ...STATES.map((a) => [
    `transitions-depuis-${a}`,
    'backdrop=1',
    async (page) => {
      for (const b of STATES.filter((x) => x !== a)) {
        await set(page, a);
        await wait(page, 1000);
        await set(page, b);
        await wait(page, 1300);
      }
    },
  ]),
  [
    'veille-vivante',
    'backdrop=1',
    async (page) => {
      // Veille seule : petites animations rares et aléatoires, puis le survol
      // réveille Kobi qui suit le curseur, puis il se rendort.
      await wait(page, 16_000);
      await page.mouse.move(120, 120);
      for (let i = 0; i <= 60; i++) {
        const a = (i / 60) * Math.PI * 2;
        await page.mouse.move(580 + Math.cos(a) * 230, 260 + Math.sin(a) * 200);
        await wait(page, 80);
      }
      await page.mouse.move(-10, 300);
      await page.evaluate(() => document.dispatchEvent(new MouseEvent('mouseout', { relatedTarget: null })));
      await wait(page, 5000);
    },
  ],
  [
    'curseur',
    'backdrop=1&state=listen',
    async (page) => {
      await wait(page, 600);
      for (const s of ['listen', 'answer', 'done']) {
        await set(page, s);
        for (let i = 0; i <= 40; i++) {
          const a = (i / 40) * Math.PI * 2;
          await page.mouse.move(560 + Math.cos(a) * 250, 260 + Math.sin(a * 2) * 180);
          await wait(page, 70);
        }
      }
      // Survol de la pilule : Kobi se fait écraser.
      await page.mouse.move(580, 260);
      await wait(page, 1200);
    },
  ],
  [
    'reunion-chargee',
    'backdrop=1',
    async (page) => {
      // Enregistrement + nouvelle question + réflexion + réponse en flux + notes
      // qui tombent pendant ce temps : tout doit rester fluide.
      const debut = Date.now() - 754_000;
      const rec = (notes, pause = false) =>
        page.evaluate(([notes, pause, debut]) => window.kobi.recording({ actif: true, pause, debut_ms: debut, notes, titre: 'Point hebdo' }), [notes, pause, debut]);
      await wait(page, 500);
      await rec(0);
      await wait(page, 1500);
      await rec(1);
      await wait(page, 1200);
      await play(page, 'listen');
      await wait(page, 2200);
      await rec(2);
      await play(page, 'think');
      await wait(page, 2600);
      await play(page, 'answer');
      for (let i = 3; i <= 5; i++) {
        await wait(page, 1300);
        await rec(i);
      }
      await wait(page, 1500);
      await play(page, 'listen');
      await wait(page, 1500);
      await play(page, 'write');
      await rec(6);
      await wait(page, 2000);
      await rec(6, true);
      await set(page, 'idle');
      await wait(page, 2000);
      await rec(6, false);
      await wait(page, 1500);
    },
  ],
  [
    'reunion-prise-de-notes',
    'backdrop=1',
    async (page) => {
      // Pendant l'enregistrement, le repos de Kobi est la prise de notes ; il
      // répond à une question puis retourne à ses notes.
      const debut = Date.now() - 754_000;
      const rec = (notes, note = null, pause = false) =>
        page.evaluate(([notes, note, pause, debut]) => window.kobi.recording({ actif: true, pause, debut_ms: debut, notes, titre: 'Point hebdo', derniere_note: note }), [notes, note, pause, debut]);
      await wait(page, 800);
      await rec(0);
      await wait(page, 3500);
      await rec(1, 'Décision : refresh token à 30 jours');
      await wait(page, 3500);
      await rec(2, 'Action : mettre à jour la doc API (Léa)');
      await wait(page, 2500);
      await play(page, 'listen');
      await wait(page, 2400);
      await play(page, 'think');
      await wait(page, 2800);
      await play(page, 'answer');
      await wait(page, 3500);
      await set(page, 'idle');
      await wait(page, 2500);
      await rec(3, 'Question : impact sur le mobile ?');
      await wait(page, 3500);
    },
  ],
  [
    'reunion-demarrage-consentement',
    'backdrop=1&state=listen',
    async (page) => {
      // Démarrage : la carte de consentement arrive et reste immobile (ses
      // boutons se cliquent), seuls son surtitre, son titre et le halo vivent.
      await wait(page, 800);
      await page.evaluate(() => window.kobi.startMeeting('Point hebdo'));
      await wait(page, 5000);
      await page.getByRole('button', { name: 'EN' }).click();
      await wait(page, 1500);
      await page.getByRole('button', { name: 'C’est fait' }).click();
      await wait(page, 3000);
    },
  ],
  [
    'lisibilite-fond-sombre',
    'backdrop=sombre',
    async (page) => {
      // La veille d'abord (pendant un enregistrement, le repos est la prise de notes).
      for (const s of STATES) {
        await play(page, s);
        if (s === 'idle') {
          await wait(page, 1800);
          await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now() - 754_000, notes: 3, titre: 'Point hebdo' }));
        }
        await wait(page, 1800);
      }
    },
  ],
  [
    'reduire-les-animations',
    'backdrop=1&reduced=1',
    async (page) => {
      for (const s of STATES) {
        await play(page, s);
        await wait(page, 1200);
      }
    },
  ],
].filter(([name]) => only.length === 0 || only.includes(name));

async function record(browser, name, query, scenario) {
  const tmp = join(out, `.tmp-${name}`);
  rmSync(tmp, { recursive: true, force: true });
  const context = await browser.newContext({ viewport: { width: W, height: H }, recordVideo: { dir: tmp, size: { width: W, height: H } } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}?${query}`);
  await page.waitForFunction(() => 'kobi' in window);
  await page.evaluate(() => document.fonts.ready);
  await scenario(page);
  await context.close();
  const webm = join(out, `${name}.webm`);
  renameSync(join(tmp, readdirSync(tmp)[0]), webm);
  rmSync(tmp, { recursive: true, force: true });
  const r = spawnSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', webm, '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '20', '-preset', 'veryfast', '-movflags', '+faststart', join(out, `${name}.mp4`)], { stdio: 'inherit' });
  if (r.status !== 0) throw new Error(`${name} : conversion MP4 impossible`);
  if (!keepWebm) rmSync(webm);
  if (errors.length) throw new Error(`${name} : erreurs dans la page\n${errors.join('\n')}`);
  console.log(`${name} : ok`);
}

mkdirSync(out, { recursive: true });
if (await fetch(base).then(() => true, () => false)) throw new Error(`port ${PORT} déjà occupé : arrêter l'ancien serveur`);
const server = spawn('node', ['node_modules/vite/bin/vite.js', 'preview', '--outDir', dist, '--port', String(PORT), '--strictPort', '--host', '127.0.0.1'], { stdio: 'ignore', detached: true });
try {
  for (let i = 0; i < 150 && !(await fetch(base).then(() => true, () => false)); i++) await new Promise((r) => setTimeout(r, 200));
  const browser = await chromium.launch();
  for (const [name, query, scenario] of CLIPS) await record(browser, name, query, scenario);
  await browser.close();
} finally {
  stopTree(server);
}
