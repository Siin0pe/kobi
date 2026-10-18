import { expect, test, type Page } from '@playwright/test';
import { expectEconomicalFont, expectNoErrors, openUi } from './helpers';

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

test('après 20 s de veille, Kobi passe en sommeil profond : le mot ondule lentement, au pas du métronome', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  const kobi = page.locator('.kobi');
  await page.clock.runFor(19_000);
  await expect(kobi).not.toHaveClass(/deep/);
  await page.clock.runFor(2_000);
  await expect(kobi).toHaveClass(/deep/);
  // Le mot vit toujours (exigence « toujours animé »), mais par paliers et lentement.
  await page.clock.runFor(1_500);
  await page.waitForTimeout(1_400);
  await expectEconomicalFont(page);
  // La respiration continue.
  expect(await page.locator('.kobi .pill').evaluate((n) => n.getAnimations().length)).toBe(1);
  // Le moindre changement d'état réveille Kobi.
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await expect(kobi).not.toHaveClass(/deep/);
  await expectNoErrors(errors);
});

test('repos économe en réunion : tout bouge encore, au rythme d’un seul métronome (4 images/s, puis 2)', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() =>
    window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 2, titre: 'Point hebdo', derniere_note: 'Décision : refresh token à 30 jours' }),
  );
  await page.clock.runFor(23_000);
  const stage = page.locator('.stage');
  await expect(stage).toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '4');
  /** Boucles CSS de la scène : nom, état, temps courant. */
  const loops = () =>
    page.evaluate(() =>
      document
        .querySelector('.scene')!
        .getAnimations({ subtree: true })
        .filter((a) => a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity)
        .map((a) => ({ name: (a as CSSAnimation).animationName, state: a.playState, t: Number(a.currentTime) })),
    );
  const before = await loops();
  // Le mot, la pilule, l'anneau, l'étiquette REC, la fiche et le champ bougent toujours…
  for (const name of ['kobi-wave', 'kobi-bob', 'kobi-scribble', 'kobi-rec', 'rec-float', 'card-float', 'say-chevron']) {
    expect(before.map((a) => a.name)).toContain(name);
  }
  // … mais aucune ne produit d'image d'elle-même : le métronome les fait avancer ensemble.
  expect(before.filter((a) => a.state !== 'paused')).toEqual([]);
  await page.clock.runFor(1_000);
  const after = await loops();
  for (let i = 0; i < before.length; i++) {
    expect(after[i]!.t - before[i]!.t, after[i]!.name).toBeGreaterThan(950);
    expect(after[i]!.t - before[i]!.t, after[i]!.name).toBeLessThan(1_050);
  }
  // Toutes avancent exactement du même pas : elles changent sur les mêmes images.
  await page.clock.runFor(300);
  const later = await loops();
  const steps = new Set(later.map((a, i) => Math.round(a.t - after[i]!.t)));
  expect(steps.size).toBe(1);
  expect([...steps][0]).toBeGreaterThan(0);
  // Après deux minutes, 2 images/s.
  await page.clock.runFor(120_000);
  await expect(stage).toHaveAttribute('data-fps', '2');
  // Le moindre changement d'état réveille tout : les boucles reprennent seules.
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await expect(stage).not.toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '0');
  expect((await loops()).filter((a) => a.state === 'paused')).toEqual([]);
  await expectNoErrors(errors);
});
