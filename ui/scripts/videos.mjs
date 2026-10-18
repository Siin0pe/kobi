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
  // KOB-149 : répertoire de la veille, pincement, fenêtre déplacée, voix, frappe.
  // Sans fond : seul Kobi flotte, comme dans l'app (fenêtre transparente).
  [
    'veille-repertoire',
    '',
    async (page) => {
      await wait(page, 800);
      const names = await page.evaluate(() => window.kobi.stage.kobi.idleNames());
      for (const n of names) {
        await page.evaluate((n) => window.kobi.stage.kobi.playIdle(n), n);
        await page.waitForFunction(() => !document.querySelector('.kobi').dataset.idle, null, { timeout: 8000 });
        await wait(page, 500);
      }
    },
  ],
  [
    'pincement-survol',
    '',
    async (page) => {
      await wait(page, 800);
      for (const s of ['idle', 'listen', 'done', 'answer']) {
        await set(page, s);
        await wait(page, 900);
        await page.mouse.move(150, 300);
        await page.mouse.move(580, 260, { steps: 12 });
        await wait(page, 700);
        await page.mouse.down();
        await wait(page, 300);
        await page.mouse.up();
        await wait(page, 1300);
        await page.mouse.move(-10, 300);
        await page.evaluate(() => document.dispatchEvent(new MouseEvent('mouseout', { relatedTarget: null })));
        await wait(page, 600);
      }
    },
  ],
  [
    'fenetre-deplacee',
    '',
    async (page) => {
      // Comme l'événement tauri://move de la fenêtre native : on secoue la fenêtre.
      await wait(page, 800);
      const jostle = (dx, dy) => page.evaluate(([dx, dy]) => window.kobi.stage.kobi.jostle(dx, dy), [dx, dy]);
      for (const [dx, dy, n] of [[18, 0, 12], [-22, 4, 10], [0, -16, 8], [25, 10, 6]]) {
        for (let i = 0; i < n; i++) {
          await jostle(dx, dy);
          await wait(page, 16);
        }
        await wait(page, 1600);
      }
    },
  ],
  [
    'ecoute-volume',
    'state=listen',
    async (page) => {
      await wait(page, 900);
      for (let i = 0; i < 160; i++) {
        const v = Math.max(0, Math.sin(i / 6) * 0.8 + (Math.random() - 0.3) * 0.4);
        await page.evaluate((v) => window.kobi.setLevel(v), v);
        await wait(page, 50);
      }
      await wait(page, 1500);
    },
  ],
  [
    'frappe-brouillee',
    '',
    async (page) => {
      await wait(page, 600);
      await page.evaluate(() => window.kobi.setState('answer', { question: 'Où est le rafraîchissement du jeton ?', answer: '' }));
      const answer = 'Dans src/auth/refresh.ts : la route /auth/refresh renouvelle le jeton toutes les 15 minutes, et le mobile garde le sien 30 jours.';
      let t = '';
      for (const w of answer.split(' ')) {
        t = t ? `${t} ${w}` : w;
        await page.evaluate((t) => window.kobi.update({ answer: t }), t);
        await wait(page, 140);
      }
      await wait(page, 3000);
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
    'conversation-clavier-historique',
    'backdrop=1',
    async (page) => {
      // KOB-133 : on tape une question (Entrée), Kobi répond ; on en tape une
      // autre ; puis on fait défiler la bulle vers le haut pour relire.
      const bulles = [];
      const echange = async (question, reponse, clavier) => {
        await page.locator('.say-input').click();
        await page.keyboard.type(question, { delay: 45 });
        await wait(page, 300);
        await page.keyboard.press('Enter');
        bulles.push({ tour: bulles.length + 1, question, reponse: '', statut: 'attente', clavier });
        await page.evaluate((b) => window.kobi.conversation(b), bulles);
        await wait(page, 900);
        await page.evaluate(() => window.kobi.setState('think', window.kobi.demoData('think')));
        await wait(page, 1600);
        await page.evaluate(() => window.kobi.setState('answer', { answer: '' }));
        let t = '';
        for (const w of reponse.split(' ')) {
          t = t ? `${t} ${w}` : w;
          await page.evaluate((t) => window.kobi.update({ answer: t }), t);
          await wait(page, 90);
        }
        bulles[bulles.length - 1] = { ...bulles[bulles.length - 1], reponse, statut: 'fini' };
        await page.evaluate((b) => window.kobi.conversation(b), bulles);
        await wait(page, 1800);
      };
      await wait(page, 600);
      await echange('où est géré le refresh token ?', 'Dans api/src/auth/refresh.ts, ligne 88. Durée actuelle : 15 min.', true);
      await echange('et côté mobile ?', 'Le mobile le renouvelle dans app/lib/session.dart, ligne 41.', true);
      await page.locator('.say-input').blur();
      await page.mouse.move(220, 250);
      await page.mouse.wheel(0, -300);
      await wait(page, 2500);
      await page.mouse.wheel(0, -300);
      await wait(page, 1500);
      await page.keyboard.press('Escape');
      await wait(page, 1500);
    },
  ],
  [
    'tache-longue',
    'backdrop=1',
    async (page) => {
      // KOB-135 : annonce immédiate, Kobi reste disponible (une autre question
      // passe), puis le résultat arrive en notification et s'ouvre d'un clic.
      const longue = 'liste tous les appels à refresh dans la codebase';
      const bulles = [{ tour: 1, question: longue, reponse: '', statut: 'attente', clavier: false }];
      await wait(page, 600);
      await page.evaluate(() => window.kobi.setState('listen', { question: 'liste tous les appels à refresh dans la codebase' }));
      await page.evaluate((b) => window.kobi.conversation(b), bulles);
      await wait(page, 1800);
      await page.evaluate(() => window.kobi.setState('think', window.kobi.demoData('think')));
      await wait(page, 1500);
      bulles[0] = { ...bulles[0], statut: 'tache', reponse: 'Je m’en occupe, je te donne le résultat dès qu’il est prêt.' };
      await page.evaluate((b) => window.kobi.conversation(b), bulles);
      await page.evaluate(() => window.kobi.setState('answer', { answer: 'Je m’en occupe, je te donne le résultat dès qu’il est prêt.' }));
      await wait(page, 3000);
      await page.evaluate(() => window.kobi.setState('idle'));
      await wait(page, 1200);
      await page.locator('.say-input').click();
      await page.keyboard.type('quelle heure est-il ?', { delay: 45 });
      await page.keyboard.press('Enter');
      bulles.push({ tour: 2, question: 'quelle heure est-il ?', reponse: 'Il est 10 h 12.', statut: 'fini', clavier: true });
      await wait(page, 900);
      await page.evaluate(() => window.kobi.setState('answer', { answer: 'Il est 10 h 12.' }));
      await page.evaluate((b) => window.kobi.conversation(b), bulles);
      await wait(page, 2500);
      await page.evaluate(() => window.kobi.setState('idle'));
      await wait(page, 1500);
      bulles[0] = { ...bulles[0], statut: 'fini', reponse: '7 appels : api/src/auth/refresh.ts (3), app/lib/session.dart (2), web/src/auth.ts (2).' };
      await page.evaluate((b) => window.kobi.conversation(b), bulles);
      await wait(page, 3500);
      await page.locator('.notif').click();
      await wait(page, 4000);
    },
  ],
  [
    'interruption-echap',
    'backdrop=1',
    async (page) => {
      // Échap : la réponse s'arrête net, la suite du flux est ignorée.
      await wait(page, 600);
      await page.evaluate(() => window.kobi.setState('answer', { answer: '' }));
      const full = 'C’est dans api/src/auth/refresh.ts, ligne 88. La durée actuelle est de quinze minutes, et un ticket ouvert en parle déjà depuis lundi dernier, avec deux propositions.';
      let t = '';
      let i = 0;
      for (const w of full.split(' ')) {
        t = t ? `${t} ${w}` : w;
        await page.evaluate((t) => window.kobi.update({ answer: t }), t);
        await wait(page, 110);
        if (++i === 9) await page.keyboard.press('Escape');
      }
      await wait(page, 2500);
      await page.evaluate(() => window.kobi.setState('idle'));
      await wait(page, 1500);
    },
  ],
  [
    'reunion-derniere-note-et-champ',
    'backdrop=1',
    async (page) => {
      // KOB-134 : enregistrement très visible, dernière note dans la fiche,
      // et le champ reste disponible pendant la réunion.
      const debut = Date.now() - 125_000;
      const rec = (notes, note) =>
        page.evaluate(([notes, note, debut]) => window.kobi.recording({ actif: true, pause: false, debut_ms: debut, notes, titre: null, derniere_note: note }), [notes, note, debut]);
      await wait(page, 600);
      await rec(0, null);
      await wait(page, 2500);
      await rec(1, 'Décision : refresh token à 30 jours');
      await wait(page, 3000);
      await rec(2, 'Action : mettre à jour la doc API (Léa)');
      await wait(page, 2000);
      await page.locator('.say-input').click();
      await page.keyboard.type('résume les décisions', { delay: 45 });
      await page.keyboard.press('Enter');
      await wait(page, 1500);
      await page.evaluate(() => window.kobi.setState('answer', { answer: 'Une décision : refresh token à 30 jours. Une action : doc API, Léa.' }));
      await wait(page, 3500);
      await page.evaluate(() => window.kobi.setState('idle'));
      await wait(page, 2000);
      await rec(3, 'Question : impact sur le mobile ?');
      await wait(page, 3000);
    },
  ],
  [
    'repos-econome',
    'backdrop=1',
    async (page) => {
      // KOB-146 : réunion au repos, 85 s. Éveillé (60 images/s), puis repos
      // économe à 20 s (3 images/s), puis 2 images/s à 80 s : tout bouge encore.
      await page.evaluate(() =>
        window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now() - 754_000, notes: 3, titre: null, derniere_note: 'Décision : refresh token à 30 jours' }),
      );
      await wait(page, 85_000);
    },
  ],
  [
    'bulles-entree-sortie',
    'backdrop=1',
    async (page) => {
      // KOB-150 : chaque carte sort de Kobi (distribuée, rebond) et y retourne
      // quand la suivante arrive ; les puces de sources sortent une à une.
      await wait(page, 600);
      for (const s of ['listen', 'think', 'answer', 'suggest', 'done', 'error', 'idle']) {
        await set(page, s);
        await wait(page, 2200);
      }
    },
  ],
  [
    'prise-de-notes-lettres',
    'backdrop=1',
    async (page) => {
      // KOB-150 : à chaque note, ses lettres volent de Kobi vers la fiche où un
      // crayon gratte ; les chiffres REC roulent, le point pulse.
      const debut = Date.now() - 125_000;
      const notes = ['Décision : budget validé', 'Action : devis (Sophie)', 'Question : date du séminaire ?', 'Décision : Annecy en juin', 'Action : réserver la salle'];
      await wait(page, 600);
      await page.evaluate((debut) => window.kobi.recording({ actif: true, pause: false, debut_ms: debut, notes: 0, titre: null, derniere_note: null }), debut);
      await wait(page, 2500);
      for (let i = 0; i < notes.length; i++) {
        await page.evaluate(([n, note, debut]) => window.kobi.recording({ actif: true, pause: false, debut_ms: debut, notes: n, titre: null, derniere_note: note }), [i + 1, notes[i], debut]);
        await wait(page, 3000);
      }
    },
  ],
  [
    'notifications-historique',
    'backdrop=1',
    async (page) => {
      // KOB-150 : les notifications partent de Kobi et y retournent ;
      // l'historique sort de Kobi, ses échanges entrent en cascade, puis il y retourne.
      const b = (tour, statut, reponse = '') => ({ tour, question: ['où est le refresh token ?', 'liste les appels à refresh', 'et côté mobile ?'][tour - 1], reponse, statut, clavier: tour === 3 });
      await wait(page, 600);
      await page.evaluate((x) => window.kobi.conversation(x), [b(1, 'fini', 'Dans api/src/auth/refresh.ts, ligne 88.')]);
      await page.evaluate((x) => window.kobi.conversation(x), [b(1, 'fini', 'Dans api/src/auth/refresh.ts, ligne 88.'), b(2, 'tache', 'Je m’en occupe.')]);
      await wait(page, 2500);
      await page.evaluate((x) => window.kobi.conversation(x), [b(1, 'fini', 'Dans api/src/auth/refresh.ts, ligne 88.'), b(2, 'fini', '7 appels : api (3), mobile (2), web (2).'), b(3, 'fini', 'Dans app/lib/session.dart, ligne 41.')]);
      await wait(page, 3000);
      await page.mouse.move(600, 260);
      await page.mouse.wheel(0, -300);
      await wait(page, 3000);
      await page.keyboard.press('Escape');
      await wait(page, 1500);
      await page.locator('.notif').first().click();
      await wait(page, 2500);
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
