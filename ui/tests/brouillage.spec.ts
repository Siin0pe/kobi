import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const NOISE = '#%&@*?!/<>{}=+$';

/** Échantillonne les yeux et le mot à chaque image pendant `ms`. */
async function sample(page: import('@playwright/test').Page, ms: number) {
  return page.evaluate(
    (ms) =>
      new Promise<Array<{ t: number; eye: string; word: string }>>((resolve) => {
        const out: Array<{ t: number; eye: string; word: string }> = [];
        const t0 = performance.now();
        const tick = (now: number) => {
          out.push({
            t: now - t0,
            eye: document.querySelector('.kobi .g')!.textContent ?? '',
            word: document.querySelector('.kobi .word')!.textContent ?? '',
          });
          if (now - t0 < ms) requestAnimationFrame(tick);
          else resolve(out);
        };
        requestAnimationFrame(tick);
      }),
    ms,
  );
}

/**
 * Même échantillonnage sur horloge simulée : chaque pas de 16 ms est avancé à la
 * main, le résultat ne dépend pas de la charge de la machine (KOB-131 : en temps
 * réel, les images affamées faisaient expirer le test).
 */
async function sampleClock(page: import('@playwright/test').Page, ms: number) {
  const out: Array<{ t: number; eye: string; word: string }> = [];
  for (let t = 0; t <= ms; t += 16) {
    out.push({
      t,
      ...(await page.evaluate(() => ({
        eye: document.querySelector('.kobi .g')!.textContent ?? '',
        word: document.querySelector('.kobi .word')!.textContent ?? '',
      }))),
    });
    await page.clock.runFor(16);
  }
  return out;
}

test('au changement d’état, yeux et mot sont brouillés puis posés lettre par lettre', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.clock.pauseAt(Date.now() + 2_000);
  await page.clock.runFor(800);
  await page.evaluate(() => window.kobi.setState('suggest', window.kobi.demoData('suggest')));
  const frames = await sampleClock(page, 1000);
  const target = 'idée !';

  // Yeux : symboles pendant ~300 ms, puis `*`.
  const early = frames.filter((f) => f.t < 250);
  expect(early.every((f) => NOISE.includes(f.eye))).toBe(true);
  expect(frames.filter((f) => f.t > 400).every((f) => f.eye === '*')).toBe(true);

  // Mot : brouillé au début, posé à la fin, et chaque lettre une fois posée ne bouge plus.
  expect(frames[0]!.word).not.toBe(target);
  expect(frames.at(-1)!.word).toBe(target);
  const settledAt = Array.from(target).map((c, i) => {
    const first = frames.findIndex((_, j) => frames.slice(j).every((g) => Array.from(g.word)[i] === c));
    return frames[first]!.t;
  });
  for (let i = 1; i < settledAt.length; i++) {
    if (target[i] === ' ') continue;
    expect(settledAt[i]!).toBeGreaterThanOrEqual(settledAt[i - 1]! - 1);
  }
  // Tout est posé en moins de 700 ms (prototype : 60 ms × (1,6 × 5 + 4)).
  expect(settledAt.at(-1)!).toBeLessThan(800);
  await expectNoErrors(errors);
});

/**
 * Journal des yeux affichés : chaque texte posé sur le premier œil, avec
 * l'heure (horloge simulée) prise à l'instant même où il est écrit. Un
 * MutationObserver la prenait à son rappel : sous charge, la fermeture était
 * datée en retard et un clignement de 120 ms semblait n'en durer que 70
 * (KOB-195).
 */
async function watchEyes(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(() => {
    const log: Array<{ t: number; eye: string; state: string }> = [];
    const kobi = document.querySelector<HTMLElement>('.kobi')!;
    const oeil = document.querySelector<HTMLElement>('.kobi .g')!;
    const texte = Object.getOwnPropertyDescriptor(Node.prototype, 'textContent')!;
    // L'état est noté avec chaque changement : l'horloge simulée continue
    // d'avancer entre deux appels du test, un clignement d'Écoute peut donc
    // tomber juste avant le passage en Réfléchit.
    Object.defineProperty(oeil, 'textContent', {
      configurable: true,
      get() {
        return texte.get!.call(this);
      },
      set(v: string) {
        texte.set!.call(this, v);
        log.push({ t: performance.now(), eye: v ?? '', state: kobi.dataset.state ?? '' });
      },
    });
    (window as unknown as { eyeLog: () => typeof log }).eyeLog = () => log.splice(0);
  });
}

async function eyeLog(page: import('@playwright/test').Page): Promise<Array<{ t: number; eye: string; state: string }>> {
  return page.evaluate(() => (window as unknown as { eyeLog: () => Array<{ t: number; eye: string; state: string }> }).eyeLog());
}

test('Kobi cligne des yeux en écoute, jamais en réflexion', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await watchEyes(page);
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await page.clock.runFor(12_000);
  const listen = (await eyeLog(page)).filter((e) => e.state === 'listen' && !'#%&@*?!/<>{}=+$'.includes(e.eye));
  const closes = listen.filter((e) => e.eye === '–');
  // Entre 2 et 5 s entre deux clignements : au moins 2 en 12 s, au plus ~10.
  expect(closes.length).toBeGreaterThanOrEqual(2);
  expect(closes.length).toBeLessThanOrEqual(10);
  // Chaque clignement dure ~120 ms puis les yeux se rouvrent en `O`.
  for (const c of closes) {
    const reopen = listen.find((e) => e.t > c.t && e.eye === 'O');
    // Un clignement commencé à la toute fin de la fenêtre n'a pas encore rouvert les yeux.
    if (!reopen && c === closes.at(-1)) continue;
    if (!reopen) throw new Error(`yeux restés fermés après ${c.t} ms`);
    expect(reopen.t - c.t).toBeGreaterThanOrEqual(100);
    expect(reopen.t - c.t).toBeLessThan(200);
  }

  await page.evaluate(() => window.kobi.setState('think', window.kobi.demoData('think')));
  await page.clock.runFor(12_000);
  const think = (await eyeLog(page)).filter((e) => e.state === 'think' && e.eye === '–');
  expect(think).toEqual([]);
  await expectNoErrors(errors);
});

test('les yeux posés, un clignement ou une expression se peint tout de suite, même si le mot se pose encore', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  // 400 ms : les yeux sont posés (5 pas de 60 ms), le mot d'écoute pas encore.
  await page.clock.runFor(400);
  const vu = await page.evaluate(() => {
    const kobi = window.kobi.stage.kobi as unknown as { scrambler: { scrambling: boolean }; pinch(): void };
    const oeil = () => document.querySelector('.kobi .g')!.textContent;
    const avant = { oeil: oeil(), brouille: kobi.scrambler.scrambling };
    kobi.pinch();
    return { ...avant, pince: oeil() };
  });
  expect(vu.brouille, 'le mot se pose encore').toBe(true);
  expect(vu.oeil).toBe('O');
  // Avant KOB-401, l'expression attendait le pas suivant du brouillage du mot.
  expect(vu.pince).toBe('>');
  await expectNoErrors(errors);
});

test('réduire les animations : ni brouillage ni clignement', async ({ page }) => {
  const errors = await openUi(page, 'reduced=1');
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  const frames = await sample(page, 300);
  expect(frames.every((f) => f.eye === '^' && f.word === 'alors')).toBe(true);
  await expectNoErrors(errors);
});
