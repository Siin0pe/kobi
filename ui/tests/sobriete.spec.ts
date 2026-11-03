import { expect, test, type Page } from '@playwright/test';
import { expectEconomicalLoops, expectNoErrors, openUi } from './helpers';

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
    await expect.poll(() => running(page), { message: state }).toBe(0);
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
  await expect(page.locator('.kobi .gaze')).toHaveAttribute('aria-hidden', 'true');
  await expect(page.locator('.kobi .word')).toHaveAttribute('aria-hidden', 'true');
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
    // Les symboles envolés (étincelles de la transition) sont retirés dès la
    // pause, pas à l'image suivante qu'une fenêtre masquée n'a pas (KOB-403).
    expect(await page.locator('.kobi .burst span').count()).toBe(0);
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

test('économie d’énergie : après 20 s de veille, sommeil profond, Kobi respire au pas du métronome', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  // Économie d'énergie activée (KOB-200) : l'ancien repos économe.
  await page.evaluate(() => window.kobiAnimations.poser({ economie: true }));
  const kobi = page.locator('.kobi');
  await page.clock.runFor(19_000);
  await expect(kobi).not.toHaveClass(/deep/);
  await page.clock.runFor(2_000);
  await expect(kobi).toHaveClass(/deep/);
  // Kobi vit toujours (exigence « toujours animé »), mais par paliers.
  await page.clock.runFor(1_500);
  await expectEconomicalLoops(page);
  // Il respire, sans mot sous la tête (KOB-204).
  expect(await page.locator('.kobi .pill').evaluate((n) => (n.getAnimations()[0] as CSSAnimation).animationName)).toBe('kobi-breathe');
  await expect(page.locator('.kobi .word')).toBeHidden();
  // Le moindre changement d'état réveille Kobi.
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await expect(kobi).not.toHaveClass(/deep/);
  await expectNoErrors(errors);
});

test('économie d’énergie en réunion : tout bouge encore, au rythme d’un seul métronome (3 images/s, puis 2)', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  // Économie d'énergie activée (KOB-200) : l'ancien repos économe.
  await page.evaluate(() => window.kobiAnimations.poser({ economie: true }));
  await page.evaluate(() =>
    window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 2, titre: 'Point hebdo', derniere_note: 'Décision : refresh token à 30 jours' }),
  );
  await page.clock.runFor(23_000);
  const stage = page.locator('.stage');
  await expect(stage).toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '3');
  // Boucles de la scène : courbe et départ (KOB-200 : en escalier, sans pause).
  const loops = () =>
    page.evaluate(() =>
      document
        .querySelector('.scene')!
        .getAnimations({ subtree: true })
        .filter((a) => a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity)
        .map((a) => ({
          name: (a as CSSAnimation).animationName,
          state: a.playState,
          easing: String(a.effect!.getTiming().easing),
          duree: Number(a.effect!.getTiming().duration),
          // Début réel : départ plus délai, à la vitesse de lecture.
          debut: a.startTime === null ? null : Number(a.startTime) + Number(a.effect!.getTiming().delay ?? 0) / (a.playbackRate || 1),
        })),
    );
  /** À `fps` images/s : chacune en escalier d'un pas par image, toutes sur la même grille. */
  const auPas = async (fps: number): Promise<void> => {
    const pas = 1000 / fps;
    const l = await loops();
    expect(l.length).toBeGreaterThan(5);
    for (const a of l) {
      expect(a.state, a.name).toBe('running');
      expect(a.easing, a.name).toBe(`steps(${Math.max(1, Math.round(a.duree / pas))})`);
      // Début réel (délai compris, KOB-219) sur la grille commune : elles
      // changent sur les mêmes images.
      if (a.debut !== null) expect(Math.abs(a.debut / pas - Math.round(a.debut / pas)), a.name).toBeLessThan(1e-3);
    }
  };
  // La pilule, l'anneau bougent toujours (plus de mot en réunion, KOB-204) ; l'étiquette REC et la fiche,
  // cliquables, restent immobiles, mais leur point rouge (clignote, onde), leur
  // surtitre, leur libellé et le crayon vivent…
  const noms = (await loops()).map((a) => a.name);
  for (const name of ['kobi-scribble', 'kobi-rec', 'rec-blink', 'rec-onde', 'kicker-bob', 'crayon-gratte']) {
    expect(noms).toContain(name);
  }
  // … à 3 images/s, toutes ensemble, sans qu'aucune soit mise en pause ni
  // avancée à la main (aucune boucle par image, KOB-240).
  await auPas(3);
  // Après une minute, 2 images/s.
  await page.clock.runFor(60_000);
  await expect(stage).toHaveAttribute('data-fps', '2');
  await auPas(2);
  // Le moindre changement d'état réveille tout : les boucles reprennent leur courbe.
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await expect(stage).not.toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '0');
  const reveil = await loops();
  expect(reveil.filter((a) => a.state === 'paused' || a.easing.startsWith('steps('))).toEqual([]);
  await expectNoErrors(errors);
});

test('économie d’énergie : au repos, un geste (animation Web) suit le métronome puis se termine normalement', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  // Économie d'énergie activée (KOB-200) : l'ancien repos économe.
  await page.evaluate(() => window.kobiAnimations.poser({ economie: true }));
  await page.clock.runFor(23_000);
  await expect(page.locator('.stage')).toHaveClass(/econome/);
  const geste = await page.evaluate(() => {
    const w = window as unknown as { done: boolean; anim: Animation };
    w.done = false;
    w.anim = document.querySelector('.kobi .pill')!.animate([{ scale: 1 }, { scale: 1.2 }], { duration: 600 });
    w.anim.onfinish = () => (w.done = true);
    return { state: w.anim.playState, easing: String(w.anim.effect!.getTiming().easing) };
  });
  // Adopté dès sa création, en escalier (3 images/s : 2 pas sur 600 ms), sans pause.
  expect(geste).toEqual({ state: 'running', easing: 'steps(2)' });
  // Il se termine normalement (onfinish).
  await page.clock.runFor(800);
  await expect.poll(() => page.evaluate(() => (window as unknown as { done: boolean }).done)).toBe(true);
  await expectNoErrors(errors);
});
