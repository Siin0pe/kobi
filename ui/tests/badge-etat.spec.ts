// KOB-204 : le badge « compte rendu prêt » accroché à la tête de Kobi, et le
// mot sous la tête réservé aux moments où Kobi informe (réglage « Afficher
// l'état en texte »).
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

const CR = { reunion: '2026-10-18-point-hebdo', resume: 'Budget validé.', decisions: 2, actions: 1, questions: 0 };

/** Carte du compte rendu repliée en badge (« Plus tard »). */
async function badge(page: Page) {
  await emit(page, 'compte_rendu', CR);
  await page.getByRole('button', { name: 'Plus tard' }).click();
  const b = page.getByRole('button', { name: 'Compte rendu prêt : ouvrir dans l’app' });
  await expect(b).toBeVisible();
  // Fin du rebond d'arrivée.
  await expect.poll(() => b.evaluate((n) => n.getAnimations().length)).toBe(0);
  return b;
}

/** Position du badge par rapport à la tête (pilule), à l'écran. */
function ecart(page: Page): Promise<{ dx: number; dy: number; largeur: number }> {
  return page.evaluate(() => {
    const t = document.querySelector('.kobi .pill')!.getBoundingClientRect();
    const b = document.querySelector('.rapport-badge')!.getBoundingClientRect();
    return { dx: b.right - t.right, dy: b.top - t.top, largeur: b.width };
  });
}

test('le badge est accroché en haut à droite de la tête et la suit', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await badge(page);
  // Dans le dessin de Kobi, pas flottant dans la fenêtre.
  await expect(page.locator('.kobi .pill .rapport-badge')).toHaveCount(1);
  const repos = await ecart(page);
  // Coin haut droit, au bord de la tête et débordant au-dessus (à l'échelle de
  // la scène ; la tête respire, d'où la marge).
  expect(repos.dx).toBeGreaterThan(-6);
  expect(repos.dx).toBeLessThan(20);
  expect(repos.dy).toBeLessThan(0);
  expect(repos.dy).toBeGreaterThan(-20);
  // Kobi bouge (il se penche en réfléchissant) : le badge garde sa place sur la tête.
  await page.evaluate(() => window.kobi.setState('work', window.kobi.demoData('work')));
  await page.waitForTimeout(700);
  const pose = await ecart(page);
  expect(Math.abs(pose.dx - repos.dx)).toBeLessThan(12);
  expect(Math.abs(pose.dy - repos.dy)).toBeLessThan(12);
  await expectNoErrors(errors);
});

test('survol : le badge se déploie et dit ce qu’il fait ; clic : ouvre le compte rendu', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const b = await badge(page);
  const texte = page.locator('.rapport-badge-texte');
  const ferme = (await ecart(page)).largeur;
  await b.hover({ force: true });
  await expect(texte).toHaveText('Compte rendu prêt · ouvrir');
  await expect.poll(async () => (await ecart(page)).largeur).toBeGreaterThan(ferme + 60);
  await b.click({ force: true });
  expect(await calls(page)).toContainEqual(['ouvrir_carnet', { reunion: CR.reunion }]);
  await expect(b).toBeHidden();
  await expectNoErrors(errors);
});

test('clic droit : le badge s’en va (plus tard), sans rien ouvrir', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const b = await badge(page);
  await b.click({ button: 'right', force: true });
  await expect(b).toBeHidden();
  expect((await calls(page)).map(([c]) => c)).not.toContain('ouvrir_carnet');
  await expectNoErrors(errors);
});

test('lecteurs d’écran : le badge reste joignable, le dessin de Kobi reste masqué', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const b = await badge(page);
  await b.focus();
  await expect(b).toBeFocused();
  // Aucun ancêtre masqué : le bouton est dans l'arbre d'accessibilité.
  expect(await b.evaluate((n) => n.closest('[aria-hidden="true"]'))).toBeNull();
  await expect(page.locator('.kobi .gaze')).toHaveAttribute('aria-hidden', 'true');
  await expectNoErrors(errors);
});

test('au repos, pas de mot sous Kobi ; un mot court quand il écoute ou réfléchit', async ({ page }) => {
  const errors = await openUi(page, '');
  const mot = page.locator('.kobi .word');
  await expect(mot).toBeHidden();
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  await expect(mot).toBeVisible();
  await expect(mot).toHaveText('j’écoute…');
  await page.evaluate(() => window.kobi.setState('think', window.kobi.demoData('think')));
  await expect(mot).toHaveText('je réfléchis…');
  await expect(mot).toBeVisible();
  // Il répond : la bulle parle, pas de mot en plus.
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  await expect(mot).toBeHidden();
  await page.evaluate(() => window.kobi.setState('idle', window.kobi.demoData('idle')));
  await expect(mot).toBeHidden();
  await expectNoErrors(errors);
});

test('réglage « Afficher l’état en texte : Non » : jamais de mot, même en écoute', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('kobi.affichage', JSON.stringify({ etat: 'aucun' })));
  const errors = await openUi(page, 'state=listen');
  await expect(page.locator('.kobi')).toHaveClass(/m-listen/);
  await expect(page.locator('.kobi .word')).toBeHidden();
  await expectNoErrors(errors);
});
