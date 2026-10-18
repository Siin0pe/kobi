import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'] as const;

/** Animations (CSS et Web Animations) en cours dans la page. */
function running(page: Page): Promise<number> {
  return page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length);
}

/** Simule une fenêtre masquée ou de nouveau visible. */
async function setHidden(page: Page, hidden: boolean): Promise<void> {
  await page.evaluate((hidden) => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (hidden ? 'hidden' : 'visible') });
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
    document.dispatchEvent(new Event('visibilitychange'));
  }, hidden);
}

test('le réglage système « réduire les animations » est suivi, même en cours de route', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors = await openUi(page, '');
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await page.waitForTimeout(100);
    expect(await running(page), state).toBe(0);
  }
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect(page.locator('.stage')).not.toHaveClass(/reduced-motion/);
  await page.evaluate(() => window.kobi.setState('think', window.kobi.demoData('think')));
  expect(await running(page)).toBeGreaterThan(0);
  await expectNoErrors(errors);
});

test('chaque état a un libellé pour les lecteurs d’écran', async ({ page }) => {
  const errors = await openUi(page, '');
  const live = page.getByRole('status');
  const spoken = new Set<string>();
  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await expect(page.getByRole('img', { name: /^Kobi : / })).toBeVisible();
    spoken.add((await live.textContent()) ?? '');
  }
  expect(spoken.size).toBe(STATES.length);
  // Le dessin de la mascotte (symboles brouillés…) est masqué aux lecteurs d'écran.
  await expect(page.locator('.kobi')).toHaveAttribute('aria-hidden', 'true');
  await expect(live).toHaveAttribute('aria-live', 'polite');
  await expectNoErrors(errors);
});

for (const state of ['idle', 'think', 'answer', 'listen'] as const) {
  test(`fenêtre masquée : tout s’arrête (${state})`, async ({ page }) => {
    await page.clock.install();
    const errors = await openUi(page, '');
    await page.evaluate((s) => window.kobi.play(s), state);
    await page.clock.runFor(1500);
    expect(await running(page)).toBeGreaterThan(0);

    await setHidden(page, true);
    expect(await running(page)).toBe(0);
    // Aucune minuterie ne modifie plus le dessin (bouche, clignements, brouillage).
    const before = await page.locator('.kobi').innerHTML();
    await page.evaluate(() => {
      (window as unknown as { mutations: number }).mutations = 0;
      new MutationObserver((r) => ((window as unknown as { mutations: number }).mutations += r.length)).observe(
        document.querySelector('.kobi')!,
        { subtree: true, childList: true, characterData: true, attributes: true },
      );
    });
    await page.clock.runFor(10_000);
    expect(await page.evaluate(() => (window as unknown as { mutations: number }).mutations)).toBe(0);
    expect(await page.locator('.kobi').innerHTML()).toBe(before);

    await setHidden(page, false);
    expect(await running(page)).toBeGreaterThan(0);
    await expectNoErrors(errors);
  });
}

test('après 20 s de veille, Kobi passe en sommeil profond sans redessiner son texte', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  const kobi = page.locator('.kobi');
  await page.clock.runFor(19_000);
  await expect(kobi).not.toHaveClass(/deep/);
  await page.clock.runFor(2_000);
  await expect(kobi).toHaveClass(/deep/);
  // Le mot se pose en douceur (animation finie), puis plus rien ne touche à la police.
  await page.clock.runFor(1_500);
  await page.waitForTimeout(1_400);
  const fontAnimations = await page.evaluate(() =>
    document
      .getAnimations()
      .filter((a) => a.playState === 'running')
      .filter((a) => (a.effect as KeyframeEffect).getKeyframes().some((k) => 'fontVariationSettings' in k)).length,
  );
  expect(fontAnimations).toBe(0);
  // La respiration continue.
  expect(await page.locator('.kobi .pill').evaluate((n) => n.getAnimations().length)).toBe(1);
  // Le moindre changement d'état réveille Kobi.
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await expect(kobi).not.toHaveClass(/deep/);
  await expectNoErrors(errors);
});
