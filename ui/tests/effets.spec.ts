// Animations deuxième passe, bulles et enregistrement (KOB-150) : tout sort de
// Kobi et y retourne, la prise de notes se voit (lettres qui volent, crayon),
// les chiffres REC roulent ; ce qui se clique ne bouge pas ; rien de tout cela
// avec « réduire les animations » (classe de Kobi, pas le média du système).
import { expect, test, type Page } from '@playwright/test';
import { emit, expectNoErrors, mockTauri, openUi } from './helpers';

/** Animations Web (pas CSS) en cours sur un élément. */
const web = (page: Page, sel: string) =>
  page.locator(sel).first().evaluate((n) => n.getAnimations().filter((a) => !(a instanceof CSSAnimation) && a.playState !== 'finished').length);

/** Boîte d'un élément relevée plusieurs fois. */
async function boxes(page: Page, sel: string): Promise<unknown[]> {
  const out = [];
  for (let i = 0; i < 5; i++) {
    out.push(await page.locator(sel).first().boundingBox());
    await page.waitForTimeout(200);
  }
  return out;
}

test('une carte sort de Kobi (distribuée, rebond) puis retourne dans Kobi quand elle est remplacée', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  const question = page.locator('[data-card=question]');
  // Arrivée : une animation Web additive part du centre de Kobi.
  const depart = await question.evaluate((n) => {
    const a = n.getAnimations().find((x) => !(x instanceof CSSAnimation))!;
    const k = (a.effect as KeyframeEffect).getKeyframes();
    return { composite: (a.effect as KeyframeEffect).composite, first: String(k[0]!.transform), last: String(k.at(-1)!.transform) };
  });
  expect(depart.composite).toBe('add');
  expect(depart.first).toMatch(/^translate\(\d+(\.\d+)?px, \d+(\.\d+)?px\) scale\(0?\.18\)/);
  expect(depart.last).toBe('none');
  await expect.poll(() => web(page, '[data-card=question]')).toBe(0);
  // Remplacée : elle se froisse vers Kobi, puis quitte le DOM.
  await page.evaluate(() => window.kobi.setState('think', window.kobi.demoData('think')));
  const leaving = page.locator('.card-question.leaving');
  await expect(leaving).toHaveCount(1);
  expect(await web(page, '.card-question.leaving')).toBe(1);
  await expect(leaving).toHaveCount(0);
  // Les puces des sources sortent de Kobi une à une.
  await expect.poll(() => page.locator('.card-sources .source').count()).toBeGreaterThan(0);
  await expectNoErrors(errors);
});

test('prise de notes : les lettres volent de Kobi vers la fiche, le crayon gratte, la fiche cliquable ne bouge pas', async ({ page }) => {
  const errors = await openUi(page, '');
  const rec = (notes: number, note: string) =>
    page.evaluate(([notes, note]) => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now() - 60_000, notes, titre: null, derniere_note: note }), [notes, note] as const);
  await rec(1, 'Décision : budget validé');
  const fiche = page.locator('[data-card=notes]');
  await expect(fiche.locator('.text')).toHaveText('Décision : budget validé');
  await expect(fiche.locator('.crayon')).toBeVisible();
  expect(await fiche.locator('.crayon').evaluate((n) => n.getAnimations().map((a) => (a as CSSAnimation).animationName))).toContain('crayon-gratte');
  await rec(2, 'Action : devis (Sophie)');
  // Les dernières lettres de la note s'envolent (7 au plus), puis disparaissent.
  await expect.poll(() => page.locator('.lettre-volante').count()).toBeGreaterThan(3);
  expect(await page.locator('.lettre-volante').count()).toBeLessThanOrEqual(7);
  await expect(page.locator('.lettre-volante')).toHaveCount(0, { timeout: 5_000 });
  await expect(fiche.locator('.text')).toHaveText('Action : devis (Sophie)');
  // La fiche s'ouvre d'un clic (notes en direct) : elle reste en place.
  expect(await fiche.evaluate((n) => n.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  const b = await boxes(page, '[data-card=notes]');
  for (const x of b) expect(x).toEqual(b[0]);
  await expectNoErrors(errors);
});

test('REC : les chiffres roulent, le texte reste « 12:34 », le point pulse, l’étiquette cliquable ne bouge pas', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now() - 754_000, notes: 2, titre: null }));
  const time = page.locator('.rec-time');
  await expect(time).toHaveText('12:34');
  expect(await time.locator('.chiffre').count()).toBe(5);
  await expect(page.locator('.rec-notes')).toHaveText('2 notes');
  // Une seconde plus tard, seul le dernier chiffre change et roule.
  await page.clock.runFor(1_000);
  await expect(time).toHaveText('12:35');
  const rolling = await time.locator('.chiffre').evaluateAll((spans) => spans.map((s) => s.getAnimations().length));
  // Sous charge, le roulement précédent peut ne pas être fini (KOB-172).
  expect(rolling.slice(0, 4)).toEqual([0, 0, 0, 0]);
  expect(rolling[4]).toBeGreaterThanOrEqual(1);
  // Le point pulse (onde), l'étiquette elle-même ne bouge pas.
  expect(await page.locator('.rec-dot').evaluate((n) => n.getAnimations({ subtree: true }).map((a) => (a as CSSAnimation).animationName))).toContain('rec-onde');
  expect(await page.locator('.rec').evaluate((n) => n.getAnimations().length)).toBe(0);
  await expectNoErrors(errors);
});

test('notifications : elles partent de Kobi et y retournent ; l’historique sort de Kobi en cascade et y retourne', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const bulle = (tour: number, statut: string, reponse = '') => ({ tour, question: `question ${tour}`, reponse, statut, clavier: false });
  await emit(page, 'conversation', [bulle(1, 'tache')]);
  const notif = page.locator('.notif[data-kind=running]');
  await expect(notif).toBeVisible();
  expect(await web(page, '.notif')).toBe(1);
  await emit(page, 'conversation', [bulle(1, 'fini', 'Résultat.')]);
  // L'annonce retourne dans Kobi pendant que la livraison en sort.
  await expect(page.locator('.notif.leaving')).toHaveCount(1);
  await expect(page.locator('.notif.leaving')).toHaveCount(0);
  await expect(page.locator('.notif[data-kind=delivered]')).toBeVisible();

  await emit(page, 'conversation', [bulle(1, 'fini', 'Résultat.'), bulle(2, 'fini', 'Deux.'), bulle(3, 'fini', 'Trois.')]);
  await page.mouse.move(600, 260);
  await page.mouse.wheel(0, -300);
  const history = page.getByRole('log', { name: 'Échanges récents avec Kobi' });
  await expect(history).toBeVisible();
  expect(await web(page, '.card-history')).toBe(1);
  expect(await page.locator('.card-history .ex').evaluateAll((items) => items.filter((i) => i.getAnimations().length > 0).length)).toBe(3);
  await page.keyboard.press('Escape');
  await expect(history).toBeHidden();
  await expectNoErrors(errors);
});

test('« réduire les animations » (classe de Kobi) : ni envol, ni lettres, ni chiffres qui roulent', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.stage.setReducedMotion(true));
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  expect(await web(page, '[data-card=question]')).toBe(0);
  const rec = (notes: number, note: string) =>
    page.evaluate(([notes, note]) => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now() - 754_000, notes, titre: null, derniere_note: note }), [notes, note] as const);
  await page.evaluate(() => window.kobi.setState('idle'));
  await rec(1, 'Décision : budget validé');
  await rec(2, 'Action : devis');
  expect(await page.locator('.lettre-volante').count()).toBe(0);
  await page.clock.runFor(1_000);
  expect(await page.locator('.rec-time .chiffre').evaluateAll((s) => s.reduce((n, x) => n + x.getAnimations().length, 0))).toBe(0);
  expect(await page.locator('.scene').evaluate((n) => n.getAnimations({ subtree: true }).filter((a) => a.playState === 'running').length)).toBe(0);
  await expectNoErrors(errors);
});

test('sobriété : une seule notification visible, les autres attendent leur tour (rappel d’abord, puis résultat, puis annonce)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const bulle = (tour: number, statut: string, reponse = '') => ({ tour, question: `question ${tour}`, reponse, statut, clavier: false });
  await emit(page, 'conversation', [bulle(1, 'tache'), bulle(2, 'tache')]);
  const visibles = page.locator('.notifs .notif:not(.leaving):visible');
  await expect(visibles).toHaveCount(1);
  await expect(visibles).toContainText('question 1');
  // Un résultat passe devant l'annonce encore en cours.
  await emit(page, 'conversation', [bulle(1, 'tache'), bulle(2, 'fini', 'Deux.')]);
  await expect(visibles).toHaveCount(1);
  await expect(visibles).toContainText('Résultat prêt');
  // Cliqué, il s'en va : l'annonce qui attendait sort de Kobi.
  await visibles.click();
  await expect(visibles).toHaveCount(1);
  await expect(visibles).toContainText('Je m’en occupe');
  await expectNoErrors(errors);
});
