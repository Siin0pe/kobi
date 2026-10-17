import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const CHOICES = ['kobi', 'gribouille', 'mercure', 'matrice', 'gelee', 'essaim'];

test('les réglages proposent les 6 skins avec un aperçu vivant', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto('/reglages.html');
  const options = page.getByRole('radio');
  await expect(options).toHaveCount(CHOICES.length);
  expect(await options.evaluateAll((n) => n.map((i) => (i as HTMLInputElement).value))).toEqual(CHOICES);
  await expect(page.getByRole('radio', { checked: true })).toHaveValue('kobi');
  // Survoler un skin le montre dans l'aperçu, qui enchaîne les états.
  await page.locator('.option[data-skin=matrice]').hover();
  await expect(page.locator('.cadre .stage')).toHaveAttribute('data-skin', 'matrice');
  const first = await page.locator('.cadre .stage').getAttribute('data-state');
  await expect(page.locator('.cadre .stage')).not.toHaveAttribute('data-state', first ?? '', { timeout: 4000 });
  expect(errors).toEqual([]);
});

test('le choix est gardé et appliqué tout de suite à la mascotte', async ({ page, context }) => {
  const errors = await openUi(page, '');
  await expect(page.locator('.stage')).toHaveAttribute('data-skin', 'kobi');

  const reglages = await context.newPage();
  await reglages.goto('/reglages.html');
  await reglages.getByRole('radio', { name: /Gelée/ }).check();
  await expect(reglages.getByRole('status')).toContainText('Gelée est maintenant l’apparence de Kobi');
  // La fenêtre de la mascotte change de skin sans rechargement.
  await expect(page.locator('.stage')).toHaveAttribute('data-skin', 'gelee');

  // Et le choix survit à un redémarrage.
  await page.reload();
  await page.waitForFunction(() => 'kobi' in window);
  await expect(page.locator('.stage')).toHaveAttribute('data-skin', 'gelee');
  await reglages.reload();
  await expect(reglages.getByRole('radio', { checked: true })).toHaveValue('gelee');
  await expectNoErrors(errors);
});
