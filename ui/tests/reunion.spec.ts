import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error', 'work'] as const;

test('l’indicateur REC reste visible pendant toute la réunion, avec sa durée', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, 'reunion=Point%20hebdo&depuis=754&carte=0');
  const rec = page.locator('.rec');
  await expect(rec).toBeVisible();
  await expect(rec).toContainText('REC · Point hebdo');
  await expect(rec.locator('.rec-time')).toHaveText('12:34');
  await page.clock.runFor(2_000);
  await expect(rec.locator('.rec-time')).toHaveText('12:36');
  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await expect(rec).toBeVisible();
  }
  // La durée ne se lit pas en boucle aux lecteurs d'écran.
  await expect(rec.locator('.rec-time')).toHaveAttribute('aria-hidden', 'true');
  expect(await rec.getAttribute('aria-live')).toBeNull();
  await page.evaluate(() => window.kobi.endMeeting());
  await expect(rec).toBeHidden();
  await expect(page.locator('.stage > [role=status]')).toHaveText('Réunion terminée.');
  await expectNoErrors(errors);
});

test('au démarrage, la carte propose un message prêt à coller, en français ou en anglais', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: 'http://127.0.0.1:' + (process.env.KOBI_UI_PORT ?? '4173') });
  const errors = await openUi(page, 'state=listen');
  await page.evaluate(() => {
    (window as unknown as { consent: unknown[] }).consent = [];
    window.kobi.stage.onConsent = (e) => (window as unknown as { consent: unknown[] }).consent.push(e);
    window.kobi.startMeeting('Point hebdo');
  });
  const card = page.getByRole('dialog', { name: 'Prévenez les participants' });
  await expect(card).toBeVisible();
  await expect(card.locator('blockquote')).toContainText('transcrit cette réunion en local');
  // La carte de la question attend derrière.
  await expect(page.locator('[data-card=question]')).toBeHidden();

  await card.getByRole('button', { name: 'EN' }).click();
  await expect(card.getByRole('button', { name: 'EN' })).toHaveAttribute('aria-pressed', 'true');
  await expect(card.locator('blockquote')).toHaveAttribute('lang', 'en');
  await expect(card.locator('blockquote')).toContainText('transcribes this meeting locally');
  await card.getByRole('button', { name: 'Copier le message' }).click();
  await expect(card.getByRole('button', { name: 'Copié ✓' })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain('Heads-up: I’m using Kobi');

  await card.getByRole('button', { name: 'C’est fait' }).click();
  await expect(card).toBeHidden();
  await expect(page.locator('[data-card=question]')).toBeVisible();
  await expect(page.locator('.rec')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { consent: unknown[] }).consent)).toEqual([
    { action: 'copy', lang: 'en' },
    { action: 'done', lang: 'en' },
  ]);
  await expectNoErrors(errors);
});

test('pause : REC devient PAUSE ; en mode discret, la pastille porte le point rouge', async ({ page }) => {
  const errors = await openUi(page, 'discret=1');
  await page.evaluate(() => window.kobi.startMeeting());
  // La carte attend une action : la pastille se déplie.
  await expect(page.locator('.scene')).toBeVisible();
  await page.getByRole('button', { name: 'C’est fait' }).click();
  await page.evaluate(() => window.kobi.setCompact(true));
  await expect(page.locator('.dot')).toHaveClass(/recording/);
  await expect(page.locator('.dot')).toHaveAttribute('aria-label', /réunion transcrite/);
  await page.evaluate(() => window.kobi.stage.updateMeeting({ paused: true }));
  await expect(page.locator('.dot')).not.toHaveClass(/recording/);
  await page.evaluate(() => window.kobi.setCompact(false));
  await expect(page.locator('.rec')).toHaveClass(/is-paused/);
  await expect(page.locator('.rec-label')).toHaveText('PAUSE');
  await expect(page.locator('.stage > [role=status]')).toHaveText('Transcription en pause.');
  await expectNoErrors(errors);
});
