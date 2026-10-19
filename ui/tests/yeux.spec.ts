// Yeux de Kobi (KOB-171) : aucun caractère ne reste affiché après un
// brouillage, une scène de veille ou une transition coupés en route. Capture
// du propriétaire : « o » et « 8 » en réunion, la bulle de sommeil (un « o »)
// restée posée sur l'œil droit après une scène interrompue.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

/** Glyphe des yeux de chaque état (LOOKS de kobi.ts). */
const EYES: Record<string, string> = { idle: '–', listen: 'O', think: '@', write: 'o', answer: '^', suggest: '*', done: '^', error: 'x' };
const STATES = Object.keys(EYES);

/** Les deux yeux finissent sur le glyphe de l'état (un clignement passe, `–`, est toléré en route). */
async function expectEyes(page: Page, state: string): Promise<void> {
  await expect
    .poll(() => page.locator('.kobi .g').allTextContents(), { timeout: 3000, message: `yeux en ${state}` })
    .toEqual([EYES[state], EYES[state]]);
}

async function setHidden(page: Page, hidden: boolean): Promise<void> {
  await page.evaluate((hidden) => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (hidden ? 'hidden' : 'visible') });
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
    document.dispatchEvent(new Event('visibilitychange'));
  }, hidden);
}

test('bulle de sommeil coupée par le début d’une réunion : rien ne reste sur l’œil droit', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.waitForTimeout(600);
  expect(await page.evaluate(() => (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle('snore'))).toBe(true);
  await page.waitForTimeout(1200);
  await expect(page.locator('.kobi .burst .fly')).toHaveCount(1);
  // La réunion démarre en pleine scène : Kobi passe à la prise de notes.
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 0, titre: null }));
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'write');
  await page.waitForTimeout(2500);
  await expect(page.locator('.kobi .burst .fly')).toHaveCount(0);
  await expectEyes(page, 'write');
  await expectNoErrors(errors);
});

test('enchaînements rapides (états, scènes, fenêtre masquée) : les deux yeux reprennent toujours le glyphe de l’état', async ({ page }) => {
  test.setTimeout(120_000);
  const errors = await openUi(page, '');
  await page.waitForTimeout(500);
  const scenes = await page.evaluate(() => (window.kobi.stage.kobi as unknown as { idleNames(): string[] }).idleNames());
  // Suite pseudo-aléatoire reproductible.
  let seed = 7;
  const rnd = (n: number) => ((seed = (seed * 1103515245 + 12345) % 2 ** 31) % n);
  for (let i = 0; i < 30; i++) {
    const state = STATES[rnd(STATES.length)]!;
    if (rnd(3) === 0) {
      // Une scène de la veille, coupée au bout de 0,2 à 1,4 s.
      await page.evaluate(() => window.kobi.setState('idle'));
      await page.waitForTimeout(150);
      await page.evaluate((n) => (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle(n), scenes[rnd(scenes.length)]!);
      await page.waitForTimeout(200 + rnd(1200));
    }
    await page.evaluate((s) => window.kobi.setState(s as never, window.kobi.demoData(s as never)), state);
    // Parfois coupé en plein brouillage (60 ms par pas), ou masqué pendant la transition.
    await page.waitForTimeout(20 + rnd(200));
    if (rnd(4) === 0) {
      await setHidden(page, true);
      await page.waitForTimeout(50);
      await setHidden(page, false);
    }
    await expectEyes(page, state);
    await expect(page.locator('.kobi .burst .fly')).toHaveCount(0);
  }
  await expectNoErrors(errors);
});
