import { expect, test } from '@playwright/test';
import { emit, expectNoErrors, mockTauri, openUi } from './helpers';

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

test('transcription en retard : REC le dit, avec le raccourci réel pour arrêter, puis l’efface au rattrapage (KOB-250)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 20 * 60_000, notes: 4, titre: null });
  const rec = page.locator('.rec');
  const retard = rec.locator('.rec-retard');
  await expect(retard).toBeHidden();
  await emit(page, 'transcription_retard', { secondes: 125, raccourci: 'Ctrl+Alt+Q' });
  await expect(retard).toHaveText('retard 2 min');
  await expect(retard).toHaveAttribute('title', 'La transcription a 2 min de retard. Pour arrêter tout de suite : Ctrl+Alt+Q.');
  // Une note arrive : le retard reste affiché.
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 20 * 60_000, notes: 5, titre: null });
  await expect(retard).toHaveText('retard 2 min');
  await emit(page, 'transcription_retard', { secondes: 20, raccourci: 'Ctrl+Shift+R' });
  await expect(retard).toHaveText('retard 20 s');
  await expect(retard).toHaveAttribute('title', /Ctrl\+Maj\+R\.$/);
  await emit(page, 'transcription_retard', null);
  await expect(retard).toBeHidden();
  await expectNoErrors(errors);
});

test('retard signalé juste avant le début de la réunion : REC l’affiche dès qu’il apparaît (KOB-250)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'transcription_retard', { secondes: 16, raccourci: 'Ctrl+Shift+R' });
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now(), notes: 0, titre: null });
  await expect(page.locator('.rec .rec-retard')).toHaveText('retard 16 s');
  await expectNoErrors(errors);
});

test('réunion terminée : ni « micro perdu », ni retard, ni son de l’ordi sur l’étiquette pendant « Annuler » (KOB-291)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 5 * 60_000, notes: 1, titre: null });
  await emit(page, 'micro', true);
  await emit(page, 'transcription_retard', { secondes: 120, raccourci: 'Ctrl+Shift+R' });
  await emit(page, 'son_ordi', true);
  const rec = page.locator('.rec');
  for (const indicateur of ['.rec-micro', '.rec-retard', '.rec-eux']) await expect(rec.locator(indicateur)).toBeVisible();
  await emit(page, 'enregistrement', { actif: false, pause: false, annulable: true, debut_ms: Date.now() - 5 * 60_000, notes: 1, titre: null });
  await expect(rec).toHaveClass(/is-ended/);
  await expect(rec.locator('.rec-label')).toHaveText('réunion terminée');
  for (const indicateur of ['.rec-micro', '.rec-retard', '.rec-eux']) await expect(rec.locator(indicateur)).toBeHidden();
  await expectNoErrors(errors);
});

test('micro perdu sans autre micro : REC le montre jusqu’à son retour, caché en pause (KOB-236)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 5 * 60_000, notes: 1, titre: null });
  const micro = page.locator('.rec .rec-micro');
  await expect(micro).toBeHidden();
  await emit(page, 'micro', true);
  await expect(micro).toHaveText('micro perdu');
  await expect(micro).toHaveAttribute('title', /votre voix n’est plus transcrite/);
  // Une note arrive : l'indicateur reste.
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 5 * 60_000, notes: 2, titre: null });
  await expect(micro).toBeVisible();
  await emit(page, 'enregistrement', { actif: true, pause: true, debut_ms: Date.now() - 5 * 60_000, notes: 2, titre: null });
  await expect(micro).toBeHidden();
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 5 * 60_000, notes: 2, titre: null });
  await expect(micro).toBeVisible();
  await emit(page, 'micro', false);
  await expect(micro).toBeHidden();
  await expectNoErrors(errors);
});
