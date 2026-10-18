// Captures du démarrage de réunion (KOB-83) : carte « prévenez les participants »,
// indicateur REC pendant les états, pastille du mode discret ; vidéo du parcours.
//   npm run build && node scripts/captures-reunion.mjs --out captures/reunion
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readdirSync, renameSync, rmSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';

const arg = (n, d) => (process.argv.includes(`--${n}`) ? process.argv[process.argv.indexOf(`--${n}`) + 1] : d);
const out = resolve(arg('out', 'captures/reunion'));
const PORT = 4187;
mkdirSync(out, { recursive: true });
const server = spawn('node', ['node_modules/vite/bin/vite.js', 'preview', '--port', String(PORT), '--strictPort', '--host', '127.0.0.1'], { stdio: 'ignore', detached: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
try {
  for (let i = 0; i < 100 && !(await fetch(`http://127.0.0.1:${PORT}/`).then(() => true, () => false)); i++) await sleep(200);
  const browser = await chromium.launch();
  const tmp = join(out, '.video');
  const context = await browser.newContext({ viewport: { width: 820, height: 560 }, recordVideo: { dir: tmp, size: { width: 820, height: 560 } } });
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const page = await context.newPage();
  await page.goto(`http://127.0.0.1:${PORT}/?backdrop=1`);
  await page.waitForFunction(() => 'kobi' in window);
  await page.evaluate(() => document.fonts.ready);
  await sleep(600);
  await page.evaluate(() => window.kobi.startMeeting('Point hebdo produit'));
  await sleep(1200);
  await page.screenshot({ path: join(out, `kobi-carte-fr.png`) });
  await page.getByRole('button', { name: 'EN' }).click();
  await sleep(700);
  await page.screenshot({ path: join(out, `kobi-carte-en.png`) });
  await page.getByRole('button', { name: 'Copier le message' }).click();
  await sleep(900);
  await page.getByRole('button', { name: 'C’est fait' }).click();
  await sleep(500);
  for (const s of ['listen', 'think', 'answer']) {
    await page.evaluate((st) => window.kobi.play(st), s);
    await sleep(2200);
  }
  await page.screenshot({ path: join(out, `kobi-rec-reponse.png`) });
  await page.evaluate(() => window.kobi.stage.updateMeeting({ paused: true }));
  await page.evaluate(() => window.kobi.setState('idle'));
  await sleep(1200);
  await page.screenshot({ path: join(out, `kobi-pause.png`) });
  await page.evaluate(() => window.kobi.stage.updateMeeting({ paused: false }));
  await page.evaluate(() => window.kobi.setCompact(true));
  await sleep(1500);
  await page.screenshot({ path: join(out, `kobi-pastille-rec.png`), clip: { x: 700, y: 440, width: 120, height: 120 } });
  await context.close();
  const [file] = readdirSync(tmp);
  renameSync(join(tmp, file), join(out, `kobi-parcours.webm`));
  rmSync(tmp, { recursive: true, force: true });
  await browser.close();
  const r = spawnSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', join(out, `kobi-parcours.webm`), '-vf', 'fps=15,scale=410:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4', join(out, `kobi-parcours.gif`)], { stdio: 'inherit' });
  if (r.status !== 0) throw new Error('ffmpeg');
  console.log('ok');
} finally {
  process.kill(-server.pid);
}
