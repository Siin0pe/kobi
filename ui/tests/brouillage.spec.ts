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
            eye: document.querySelector('.skin-kobi .g')!.textContent ?? '',
            word: document.querySelector('.skin-kobi .word')!.textContent ?? '',
          });
          if (now - t0 < ms) requestAnimationFrame(tick);
          else resolve(out);
        };
        requestAnimationFrame(tick);
      }),
    ms,
  );
}

test('au changement d’état, yeux et mot sont brouillés puis posés lettre par lettre', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi');
  await page.waitForTimeout(800);
  await page.evaluate(() => window.kobi.setState('suggest', window.kobi.demoData('suggest')));
  const frames = await sample(page, 1000);
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

/** Journal des yeux affichés : chaque remplacement du texte, avec l'heure (horloge simulée). */
async function watchEyes(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(() => {
    const log: Array<{ t: number; eye: string }> = [];
    (window as unknown as { eyeLog: typeof log }).eyeLog = log;
    new MutationObserver((records) => {
      for (const r of records) for (const n of r.addedNodes) log.push({ t: performance.now(), eye: n.textContent ?? '' });
    }).observe(document.querySelector('.skin-kobi .g')!, { childList: true });
  });
}

async function eyeLog(page: import('@playwright/test').Page): Promise<Array<{ t: number; eye: string }>> {
  return page.evaluate(() => (window as unknown as { eyeLog: Array<{ t: number; eye: string }> }).eyeLog.splice(0));
}

test('Kobi cligne des yeux en écoute, jamais en réflexion', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, 'skin=kobi');
  await watchEyes(page);
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await page.clock.runFor(12_000);
  const listen = (await eyeLog(page)).filter((e) => !'#%&@*?!/<>{}=+$'.includes(e.eye));
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
  const think = (await eyeLog(page)).filter((e) => e.eye === '–');
  expect(think).toEqual([]);
  await expectNoErrors(errors);
});

test('réduire les animations : ni brouillage ni clignement', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi&reduced=1');
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  const frames = await sample(page, 300);
  expect(frames.every((f) => f.eye === '^' && f.word === 'alors')).toBe(true);
  await expectNoErrors(errors);
});
