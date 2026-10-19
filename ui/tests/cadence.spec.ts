// Images par seconde (KOB-200) : réglage 5 / 15 / 30 / 60 / 120 / suivre
// l'écran, défaut 60, appliqué à chaud ; « Économie d'énergie » (désactivée
// par défaut) plafonne au repos prolongé. La cadence se mesure sur l'horloge
// simulée : combien de fois une boucle tenue avance en une seconde.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, mockTauri, openUi } from './helpers';

type Fps = 5 | 15 | 30 | 60 | 120 | 'ecran';

const poser = (page: Page, r: { fps?: Fps; economie?: boolean }) => page.evaluate((r) => window.kobiAnimations.poser(r), r);
const ecran = (page: Page, hz: number) => page.evaluate((hz) => window.kobiAnimations.ecran(hz), hz);

/** Boucles CSS infinies de la scène, avec leur état. */
const boucles = (page: Page) =>
  page.evaluate(() =>
    document
      .querySelector('.stage')!
      .getAnimations({ subtree: true })
      .filter((a) => a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity)
      .map((a) => a.playState),
  );

/**
 * Images par seconde mesurées : nombre de fois où l'ondulation du mot (tenue
 * par le métronome) change de temps, relevé toutes les 5 ms d'horloge
 * simulée, rapporté au temps écoulé dans la page (l'horloge installée avance
 * aussi d'elle-même pendant les relevés).
 */
async function images(page: Page, ms: number): Promise<number> {
  await page.evaluate(() => {
    const w = window as unknown as { suivie?: Animation };
    w.suivie = document
      .querySelector('.stage')!
      .getAnimations({ subtree: true })
      .find((a) => a instanceof CSSAnimation && a.animationName === 'kobi-breathe');
  });
  const releve = () => page.evaluate(() => [Number((window as unknown as { suivie: Animation }).suivie.currentTime), performance.now()]);
  let [avant, debut] = await releve();
  let fin = debut!;
  let n = 0;
  for (let t = 0; t < ms; t += 5) {
    await page.clock.runFor(5);
    const [maintenant, quand] = await releve();
    if (maintenant !== avant) n++;
    avant = maintenant;
    fin = quand!;
  }
  return (n * 1000) / (fin - debut!);
}

/** La cadence mesurée suit le réglage, à 20 % près (relevés de 5 ms). */
function attendre(mesure: number, fps: number): void {
  expect(mesure).toBeGreaterThanOrEqual(fps * 0.8);
  expect(mesure).toBeLessThanOrEqual(fps * 1.2 + 0.5);
}

test('par défaut, 60 images/s : rien n’est tenu, même au sommeil profond', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  const stage = page.locator('.stage');
  await expect(stage).toHaveAttribute('data-cadence', '60');
  await expect(stage).toHaveAttribute('data-fps', '0');
  await page.clock.runFor(23_000);
  await expect(page.locator('.kobi')).toHaveClass(/deep/);
  // Plus de repos économe par défaut : le navigateur anime tout lui-même.
  await expect(stage).not.toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '0');
  // (Plus de mot au repos, KOB-204 : la respiration et l'œil vivent.)
  const etats = await boucles(page);
  expect(etats.length).toBeGreaterThan(1);
  expect(etats.filter((s) => s === 'paused')).toEqual([]);
  await expectNoErrors(errors);
});

test('quand il s’affiche, le mot ondule sans aucune graisse animée (ni mise en page par image)', async ({ page }) => {
  const errors = await openUi(page, 'state=think');
  await page.waitForTimeout(1_000);
  const graisses = await page.evaluate(() =>
    document
      .querySelector('.kobi .word')!
      .getAnimations({ subtree: true })
      .filter((a) => a.playState === 'running')
      .filter((a) => (a.effect as KeyframeEffect).getKeyframes().some((k) => 'fontVariationSettings' in k || 'fontWeight' in k))
      .map((a) => (a as CSSAnimation).animationName ?? 'WAAPI'),
  );
  expect(graisses).toEqual([]);
  // L'ondulation passe par deux couches de la lettre, en fondu.
  const couches = await page.evaluate(() => document.getAnimations().filter((a) => a instanceof CSSAnimation && ['kobi-fin', 'kobi-gras'].includes(a.animationName)).length);
  expect(couches).toBeGreaterThan(0);
  await expectNoErrors(errors);
});

// KOB-240 : cadence réglée sans effet tant que KOB-200 ne la refait pas sans
// boucle par image ; ces tests reviendront avec elle.
for (const fps of [5, 15, 30] as const) {
  test.fixme(`${fps} images/s : tout avance ensemble, ${fps} fois par seconde`, async ({ page }) => {
    await page.clock.install();
    const errors = await openUi(page, '');
    await poser(page, { fps });
    const stage = page.locator('.stage');
    await expect(stage).toHaveAttribute('data-fps', String(fps));
    await expect(stage).toHaveAttribute('data-cadence', String(fps));
    // Toutes les boucles sont tenues par le métronome.
    expect((await boucles(page)).filter((s) => s !== 'paused')).toEqual([]);
    attendre(await images(page, 1_000), fps);
    await expectNoErrors(errors);
  });
}

test('60, 120 et « suivre l’écran » : l’écran donne la cadence (60 Hz, puis 120 Hz)', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  const stage = page.locator('.stage');
  await ecran(page, 60);
  for (const fps of [60, 120, 'ecran'] as const) {
    await poser(page, { fps });
    // Pas plus d'images que l'écran : à 60 Hz, 120 vaut 60.
    await expect(stage).toHaveAttribute('data-cadence', '60');
    await expect(stage).toHaveAttribute('data-fps', '0');
  }
  // Écran à 120 Hz : le navigateur anime toujours lui-même, 60 compris
  // (KOB-240 : tenu par le métronome, tout se repeignait à chaque image).
  await ecran(page, 120);
  await expect(stage).toHaveAttribute('data-cadence', '120');
  await expect(stage).toHaveAttribute('data-fps', '0');
  await poser(page, { fps: 120 });
  await expect(stage).toHaveAttribute('data-fps', '0');
  await poser(page, { fps: 60 });
  await expect(stage).toHaveAttribute('data-fps', '0');
  await expect(stage).toHaveAttribute('data-cadence', '60');
  await expectNoErrors(errors);
});

// KOB-240 : une cadence réglée ne tient plus les boucles (le métronome les
// faisait avancer à chaque image, repeint continu) ; le réglage reste
// enregistré, sans effet, jusqu'à ce que KOB-200 le refasse sans boucle.
test('réglage bas (30, 5) : rien n’est tenu, le navigateur anime lui-même', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  const stage = page.locator('.stage');
  for (const fps of [30, 5, 60] as const) {
    await poser(page, { fps });
    await page.clock.runFor(100);
    expect((await boucles(page)).filter((s) => s === 'paused')).toEqual([]);
    await expect(stage).toHaveAttribute('data-fps', '0');
    await expect(stage).toHaveAttribute('data-cadence', String(fps));
  }
  await expectNoErrors(errors);
});

test('économie d’énergie : au repos prolongé, elle plafonne la cadence réglée (3, puis 2)', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  const stage = page.locator('.stage');
  await poser(page, { fps: 30, economie: true });
  // Cadence réglée sans effet (KOB-240) : le navigateur anime d'abord.
  await expect(stage).toHaveAttribute('data-fps', '0');
  await page.clock.runFor(23_000);
  await expect(stage).toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '3');
  await page.clock.runFor(61_000);
  await expect(stage).toHaveAttribute('data-fps', '2');
  // Désactivée à chaud : la cadence réglée revient.
  await poser(page, { economie: false });
  await expect(stage).not.toHaveClass(/econome/);
  await expect(stage).toHaveAttribute('data-fps', '0');
  await expectNoErrors(errors);
});

test('réglage lu dans config.toml au chargement, puis suivi (kobi config set animations.fps)', async ({ page }) => {
  await mockTauri(page, { reglage_animations: { fps: 15, economie: false } });
  const errors = await openUi(page, '');
  const stage = page.locator('.stage');
  // Réglage lu et suivi (data-cadence) ; sans effet sur le métronome (KOB-240).
  await expect(stage).toHaveAttribute('data-cadence', '15');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('reglage', { key: 'animations.fps', value: 30 }));
  await expect(stage).toHaveAttribute('data-cadence', '30');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('reglage', { key: 'animations.fps', value: 'ecran' }));
  await expect(stage).toHaveAttribute('data-cadence', '60');
  await expect(stage).toHaveAttribute('data-fps', '0');
  await expectNoErrors(errors);
});

test('réglage dans l’app : « Images par seconde » et « Économie d’énergie », appliqués à l’app elle-même', async ({ page }) => {
  await mockTauri(page, { reglage_animations: { fps: 60, economie: false }, regler_animations: { fps: 15, economie: false } });
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto('/app.html#reglages/animations');
  const fps = page.getByRole('radiogroup', { name: 'Images par seconde' });
  await expect(fps.getByRole('radio')).toHaveCount(6);
  await expect(fps.getByRole('radio', { name: /60 images\/s/ })).toHaveAttribute('aria-checked', 'true');
  await expect(page.getByRole('switch', { name: 'Économie d’énergie' })).toHaveAttribute('aria-checked', 'false');
  await fps.getByRole('radio', { name: /15 images\/s/ }).click();
  const appels = await page.evaluate(() => (window as unknown as { __calls: Array<[string, unknown]> }).__calls.filter(([c]) => c === 'regler_animations'));
  expect(appels).toEqual([['regler_animations', { cle: 'fps', valeur: 15 }]]);
  await expect(fps.getByRole('radio', { name: /15 images\/s/ })).toHaveAttribute('aria-checked', 'true');
  // L'app suit aussi le réglage (sans effet sur son métronome, KOB-240).
  await expect(page.locator('html')).toHaveAttribute('data-cadence', '15');
  await expect(page.locator('html')).toHaveAttribute('data-fps', '0');
  await expectNoErrors(errors);
});
