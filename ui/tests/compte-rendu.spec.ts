// Compte rendu de fin de réunion (KOB-74) : la carte de Kobi, branchée sur
// l'événement `compte_rendu` de la coque (simulée), ouvre le Carnet sur la réunion.
import { expect, test } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

const CR = {
  reunion: '2026-10-18-point-hebdo',
  resume: 'Budget du séminaire validé.\nLieu retenu : Annecy.',
  decisions: 2,
  actions: 1,
  questions: 0,
};

test('fin de réunion : la carte montre le résumé et ouvre le Carnet sur la réunion', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 60_000, notes: 3, titre: null, derniere_note: 'Budget validé' });
  await emit(page, 'enregistrement', { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null, derniere_note: null });
  await emit(page, 'compte_rendu', CR);
  const card = page.getByRole('dialog', { name: 'Compte rendu prêt' });
  await expect(card).toBeVisible();
  await expect(card.locator('.rapport-resume')).toHaveText(CR.resume);
  await expect(card.locator('.rapport-chip')).toHaveText(['2 décisions', '1 action', 'aucune question']);
  await expect(page.locator('.stage > [role=status]')).toHaveText('Compte rendu de la réunion prêt.');
  // La carte ne flotte pas : le bouton se clique du premier coup.
  await card.getByRole('button', { name: 'Ouvrir dans le Carnet' }).click();
  expect(await calls(page)).toContainEqual(['ouvrir_carnet', { reunion: CR.reunion }]);
  await expect(card).toBeHidden();
  await expect(page.locator('.rapport-badge')).toBeHidden();
  await expectNoErrors(errors);
});

test('« Plus tard » ou une nouvelle question replie la carte en badge sur la tête de Kobi, qui l’ouvre ensuite', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'compte_rendu', CR);
  const card = page.getByRole('dialog', { name: 'Compte rendu prêt' });
  await card.getByRole('button', { name: 'Plus tard' }).click();
  await expect(card).toBeHidden();
  const pill = page.getByRole('button', { name: 'Compte rendu prêt : ouvrir dans l’app' });
  await expect(pill).toBeVisible();
  // De nouveau la carte, puis une question : elle se replie d'elle-même.
  await emit(page, 'compte_rendu', CR);
  await expect(card).toBeVisible();
  await emit(page, 'state', { etat: 'veille', activite: 'ecoute', mascotte: 'listen', transcrit: false });
  await expect(card).toBeHidden();
  await expect(pill).toBeVisible();
  // Accroché à la tête qui bouge (KOB-204) : jamais « immobile », d'où le
  // clic forcé, une fois fini son rebond d'arrivée.
  await expect.poll(() => pill.evaluate((n) => n.getAnimations().length)).toBe(0);
  await pill.click({ force: true });
  await expect.poll(() => calls(page)).toContainEqual(['ouvrir_carnet', { reunion: CR.reunion }]);
  await expect(pill).toBeHidden();
  await expectNoErrors(errors);
});

test('une nouvelle réunion retire l’ancien compte rendu (il reste dans le Carnet)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'compte_rendu', CR);
  await page.getByRole('button', { name: 'Plus tard' }).click();
  await expect(page.locator('.rapport-badge')).toBeVisible();
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now(), notes: 0, titre: null, derniere_note: null });
  await expect(page.locator('.rapport-badge')).toBeHidden();
  await expectNoErrors(errors);
});

test('toujours animée sans bouger ce qui se clique ; « réduire les animations » coupe tout', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'compte_rendu', CR);
  const card = page.locator('.card-rapport');
  const running = (sel: string, subtree: boolean) =>
    page.locator(sel).evaluate((n, subtree) => n.getAnimations({ subtree }).filter((a) => a.playState === 'running').length, subtree);
  // La carte a fini d'arriver.
  await expect.poll(() => running('.card-rapport', false)).toBe(0);
  expect(await running('.card-rapport', true)).toBeGreaterThanOrEqual(4);
  expect(await running('.card-rapport', false)).toBe(0);
  expect(await running('.card-rapport .btn-primary', false)).toBe(0);
  // Réglage « Animations : réduites » de Kobi : la classe reduced-motion (pas le média du système).
  await page.evaluate(() => window.kobi.stage.setReducedMotion(true));
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  expect(await running('.card-rapport', true)).toBe(0);
  await expect(card).toBeVisible();
  await expectNoErrors(errors);
});

test('sobriété : une seule chose à la fois en plus de Kobi et de sa bulle (carte, badge, notification)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'compte_rendu', CR);
  // Carte ouverte : une tâche longue annoncée attend derrière.
  await emit(page, 'conversation', [{ tour: 1, question: 'liste les appels', reponse: '', statut: 'tache', clavier: false }]);
  await expect(page.locator('.notifs')).toBeHidden();
  // Carte repliée : la notification passe, le badge s'efface tant qu'elle est là.
  await page.getByRole('button', { name: 'Plus tard' }).click();
  await expect(page.locator('.notif[data-kind=running]')).toBeVisible();
  await expect(page.locator('.rapport-badge')).toBeHidden();
  // Plus de notification : le badge revient.
  await emit(page, 'conversation', [{ tour: 1, question: 'liste les appels', reponse: 'Fini.', statut: 'fini', clavier: false }]);
  await page.locator('.notif[data-kind=delivered]').click();
  await expect(page.locator('.notif')).toHaveCount(0);
  await expect(page.locator('.rapport-badge')).toBeVisible();
  await expectNoErrors(errors);
});
