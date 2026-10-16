import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const ANSWER = 'C’est dans api/src/auth/refresh.ts, ligne 88. Durée actuelle : 15 min. Un ticket ouvert en parle déjà.';

test('la réponse en flux se tape sans jamais reculer', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi');
  await page.evaluate(() => window.kobi.play('answer'));
  const text = page.locator('[data-card=answer] .text');
  const lengths: number[] = [];
  for (let i = 0; i < 25; i++) {
    lengths.push(((await text.textContent()) ?? '').length);
    await page.waitForTimeout(120);
  }
  expect(lengths[0]).toBeLessThan(10);
  for (let i = 1; i < lengths.length; i++) expect(lengths[i]).toBeGreaterThanOrEqual(lengths[i - 1]!);
  await expect(page.locator('[data-card=answer]')).toHaveAttribute('data-typing', '0', { timeout: 10_000 });
  await expect(text).toHaveText(ANSWER);
  await expectNoErrors(errors);
});

test('réduire les animations : le texte apparaît d’un coup', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi&reduced=1');
  await page.evaluate(() => window.kobi.setState('write', window.kobi.demoData('write')));
  await expect(page.locator('[data-card=notes] .text')).toHaveText(/Question : impact sur le mobile \?$/, { timeout: 200 });
  await expectNoErrors(errors);
});

test('les sources apparaissent une à une puis sont trouvées', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi');
  await page.evaluate(() => window.kobi.play('think'));
  const pills = page.locator('[data-card=sources] .source');
  await expect(pills).toHaveCount(0);
  await expect(pills).toHaveCount(1);
  await expect(pills.nth(0)).toHaveText(/codebase …/);
  await expect(pills.nth(0)).toHaveClass(/found/);
  await expect(pills).toHaveCount(3, { timeout: 5_000 });
  await expect(pills.nth(1)).toHaveText(/linear · trouvé/);
  await expectNoErrors(errors);
});

test('la carte remplacée s’efface puis disparaît', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi&state=listen');
  await page.evaluate(() => window.kobi.setState('write', window.kobi.demoData('write')));
  await expect(page.locator('.card-question.leaving')).toHaveCount(1);
  await expect(page.locator('.card-question')).toHaveCount(0, { timeout: 1_000 });
  await expect(page.locator('[data-card]')).toHaveCount(1);
  await expectNoErrors(errors);
});

test('le compte à rebours de l’erreur met la carte à jour', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi');
  await page.evaluate(() => window.kobi.play('error'));
  const message = page.locator('[data-card=error] .message');
  await expect(message).toContainText('5 s');
  await expect(message).toContainText('4 s', { timeout: 2_000 });
  await expectNoErrors(errors);
});

test('la carte confirmée remplace les boutons', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi&state=suggest');
  await expect(page.getByRole('button', { name: 'Pas maintenant' })).toBeVisible();
  await page.evaluate(() => window.kobi.setState('done', window.kobi.demoData('done')));
  const card = page.locator('[data-card=action]');
  await expect(card).toHaveAttribute('data-status', 'confirmed');
  await expect(card).toContainText('Ticket créé.');
  await expect(card.getByRole('button')).toHaveCount(0);
  await expectNoErrors(errors);
});
