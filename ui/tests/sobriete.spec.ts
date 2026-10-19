import { expect, test, type Page } from '@playwright/test';
import { expectEconomicalFont, expectNoErrors, openUi } from './helpers';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error', 'work'] as const;

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

test('en mode « comme Windows », le réglage système « réduire les animations » est suivi, même en cours de route', async ({ page }) => {
  // Réglage « Animations » de Kobi sur « comme Windows » (par défaut : toujours).
  await page.addInitScript(() => localStorage.setItem('kobi.animations', 'systeme'));
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

test('repos économe en réunion : tout bouge encore, au rythme d’un seul métronome (3 images/s, puis 2)', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() =>
    window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 2, titre: 'Point hebdo', derniere_note: 'Décision : refresh token à 30 jours' }),
  );
  await page.clock.runFor(23_000);
  const stage = page.locator('.stage');
  await expect(stage).toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '3');
  // Les boucles relevées sont gardées dans la page : on suit les mêmes objets
  // (une boucle relancée par un petit événement de vie est un nouvel objet).
  const loops = () =>
    page.evaluate(() => {
      const w = window as unknown as { loops?: Animation[] };
      w.loops ??= document
        .querySelector('.scene')!
        .getAnimations({ subtree: true })
        .filter((a) => a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity);
      return w.loops.map((a) => ({ name: (a as CSSAnimation).animationName, state: a.playState, t: Number(a.currentTime) }));
    });
  const before = await loops();
  // Le mot, la pilule, l'anneau bougent toujours ; l'étiquette REC et la fiche,
  // cliquables, restent immobiles, mais leur point rouge (clignote, onde), leur
  // surtitre, leur libellé et le crayon vivent…
  for (const name of ['kobi-wave', 'kobi-bob', 'kobi-scribble', 'kobi-rec', 'rec-blink', 'rec-onde', 'kicker-bob', 'crayon-gratte']) {
    expect(before.map((a) => a.name)).toContain(name);
  }
  // … mais aucune ne produit d'image d'elle-même : le métronome les fait avancer ensemble.
  expect(before.filter((a) => a.state !== 'paused')).toEqual([]);
  await page.clock.runFor(2_000);
  const after = await loops();
  // Toujours tenues (pas relancées entre-temps), elles ont avancé de deux
  // secondes, à un tic près : le métronome (3 images/s) ne les avance qu'à
  // ses tics, et une fenêtre de 2 000 ms d'horloge en contient 6 ou 7 selon
  // la phase (avance mesurée 2 000 ou 2 333 ms ; 1 000 ou 1 333 sur 1 s).
  const TIC = 1000 / 3;
  const kept = after.map((a, i) => ({ ...a, d: a.t - before[i]!.t })).filter((a) => a.state === 'paused');
  expect(kept.length).toBeGreaterThan(5);
  for (const a of kept) {
    expect(Math.abs(a.d - 2_000), a.name).toBeLessThanOrEqual(TIC + 1);
  }
  // Et toutes du même pas (mêmes tics) : une seule valeur d'avance.
  expect(new Set(kept.map((a) => Math.round(a.d))).size).toBe(1);
  // Toutes avancent exactement du même pas : elles changent sur les mêmes images.
  // (400 ms : au moins un tic à 3 images/s, quelle que soit la phase.)
  await page.clock.runFor(400);
  const later = await loops();
  const steps = new Set(later.map((a, i) => (a.state === 'paused' && after[i]!.state === 'paused' ? Math.round(a.t - after[i]!.t) : null)).filter((d) => d !== null));
  expect(steps.size).toBe(1);
  expect([...steps][0]).toBeGreaterThan(0);
  // Après une minute, 2 images/s.
  await page.clock.runFor(60_000);
  await expect(stage).toHaveAttribute('data-fps', '2');
  // Le moindre changement d'état réveille tout : les boucles reprennent seules.
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await expect(stage).not.toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '0');
  expect((await loops()).filter((a) => a.state === 'paused')).toEqual([]);
  await expectNoErrors(errors);
});

test('au repos, un geste (animation Web) suit le métronome puis se termine normalement', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.clock.runFor(23_000);
  await expect(page.locator('.stage')).toHaveClass(/econome/);
  const state = await page.evaluate(() => {
    const w = window as unknown as { done: boolean; anim: Animation };
    w.done = false;
    w.anim = document.querySelector('.kobi .ch')!.animate([{ scale: 1 }, { scale: 1.2 }], { duration: 600 });
    w.anim.onfinish = () => (w.done = true);
    return w.anim.playState;
  });
  // Tenu par le métronome dès sa création : aucune image d'elle-même.
  expect(state).toBe('paused');
  await page.clock.runFor(400);
  expect(await page.evaluate(() => (window as unknown as { anim: Animation }).anim.playState)).toBe('paused');
  // Arrivé au bout, il est relâché et finit (onfinish).
  await page.clock.runFor(800);
  await expect.poll(() => page.evaluate(() => (window as unknown as { done: boolean }).done)).toBe(true);
  await expectNoErrors(errors);
});
