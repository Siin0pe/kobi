// Rappels (KOB-145) : à l'heure dite, la coque envoie l'événement `rappel` ;
// la notification sonne et reste jusqu'au clic ; la coque le dit à voix haute.
// Pas de bulle « Rappel : … » en plus (KOB-176) : elle répétait la notification.
import { expect, test } from '@playwright/test';
import { emit, expectNoErrors, mockTauri, openUi } from './helpers';

const RAPPEL = { id: 'r1', texte: "boire de l'eau", echeance_ms: new Date(2026, 9, 18, 15, 30).getTime() };

test('un rappel arrive : notification qui sonne, sans bulle en double', async ({ page }) => {
  await page.clock.install();
  await mockTauri(page);
  const errors = await openUi(page, '');
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'idle');
  await emit(page, 'rappel', RAPPEL);

  const notif = page.locator('.notif-reminder');
  await expect(notif).toBeVisible();
  await expect(notif.locator('.notif-kicker')).toHaveText('Rappel · 15:30');
  await expect(notif.locator('.notif-text')).toHaveText("boire de l'eau");
  await expect(notif).toHaveAttribute('aria-label', "Rappel : boire de l'eau");
  // La cloche sonne.
  expect(await notif.locator('.notif-icon').evaluate((e) => getComputedStyle(e).animationName)).toBe('notif-ring');
  // Une seule chose à l'écran à côté de Kobi : la notification, pas de bulle qui la répète.
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'idle');
  await expect(page.locator('[data-card=answer]')).toBeHidden();

  // Un rappel ne s'en va pas tout seul (contrairement aux tâches livrées) : seulement au clic.
  await page.clock.fastForward(60_000);
  await expect(notif).toBeVisible();
  await notif.click();
  await expect(notif).toHaveCount(0);
  await expectNoErrors(errors);
});

test('rappel sans heure, puis le même rappel une seconde fois : une seule notification', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.rappel({ id: 'r2', texte: 'appeler Léa', echeance_ms: null }));
  await page.evaluate(() => window.kobi.rappel({ id: 'r2', texte: 'appeler Léa', echeance_ms: null }));
  await expect(page.locator('.notif-reminder')).toHaveCount(1);
  await expect(page.locator('.notif-reminder .notif-kicker')).toHaveText('Rappel');
  await expectNoErrors(errors);
});

test('réduire les animations : la cloche ne bouge pas', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, 'reduced=1');
  await emit(page, 'rappel', RAPPEL);
  const icon = page.locator('.notif-reminder .notif-icon');
  await expect(icon).toBeVisible();
  expect(await icon.evaluate((e) => getComputedStyle(e).animationName)).toBe('none');
  await expectNoErrors(errors);
});
