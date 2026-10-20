// Fin de réunion (KOB-166) : dès Ctrl+Shift+R, REC s'éteint avec une petite
// animation de fin ; Kobi rédige le compte rendu (état `compose`, distinct de
// la prise de notes) avec la bulle « Je rédige le compte rendu… » ; puis la
// carte « Compte rendu prêt ». La prise de notes (`write`) ne s'affiche
// jamais hors enregistrement.
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

const CR = { reunion: '2026-10-18-reunion', resume: 'Budget validé.', decisions: 1, actions: 0, questions: 0 };
const ARRET = { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null, derniere_note: null };

/** États affichés par la scène, notés à chaque changement. */
async function suivreEtats(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as { etats: string[] };
    w.etats = [];
    const stage = document.querySelector('.stage') as HTMLElement;
    new MutationObserver(() => w.etats.push(stage.dataset.state ?? '')).observe(stage, { attributes: true, attributeFilter: ['data-state'] });
  });
}

/** Comme la coque : réunion de 7 min en prise de notes, puis fin demandée. */
async function reunionPuisFin(page: Page): Promise<void> {
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 7 * 60_000, notes: 3, titre: null, derniere_note: 'Budget validé' });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'write');
  await suivreEtats(page);
  // Ctrl+Shift+R : la session passe à « fin », l'enregistrement est annoncé arrêté.
  await emit(page, 'state', { etat: 'fin', activite: null, mascotte: 'compose', transcrit: false });
  await emit(page, 'enregistrement', ARRET);
}

test('fin de réunion : REC s’éteint tout de suite, Kobi rédige, puis la carte « Compte rendu prêt »', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await reunionPuisFin(page);

  // REC ne dit plus REC : grisé, « réunion terminée · 7 min », puis il s'efface.
  const rec = page.locator('.rec');
  await expect(rec).toHaveClass(/is-ended/);
  await expect(rec).toContainText('réunion terminée');
  await expect(rec.locator('.rec-time')).toHaveText('7 min');
  await expect(rec).not.toContainText('REC');
  await expect(page.locator('.kobi')).not.toHaveClass(/(^| )recording( |$)/);
  await expect(rec).toBeHidden({ timeout: 4000 });

  // Rédaction : état distinct, bulle « Je rédige le compte rendu… », feuilles qui s'empilent.
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'compose');
  await expect(page.locator('.kobi .sheets span').first()).toBeVisible();
  await expect(page.locator('[data-card=compose] .text')).toHaveText('Je rédige le compte rendu…');
  await expect(page.locator('[data-card=notes]')).toHaveCount(0);
  // Une seule chose à la fois avec Kobi : la bulle de rédaction seule.
  await expect(page.locator('.bubbles .card')).toHaveCount(1);

  // Compte rendu écrit : la session revient en veille, la carte arrive.
  await emit(page, 'state', { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false });
  await emit(page, 'compte_rendu', CR);
  await expect(page.getByRole('dialog', { name: 'Compte rendu prêt' })).toBeVisible();
  await expect(page.locator('[data-card=compose]')).toHaveCount(0);

  // Jamais la prise de notes après la fin de l'enregistrement.
  const etats = await page.evaluate(() => (window as unknown as { etats: string[] }).etats);
  expect(etats).toContain('compose');
  expect(etats).not.toContain('write');
  await expectNoErrors(errors);
});

test('fin de réunion avec « réduire les animations » : mêmes étapes, sans mouvement', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('kobi.animations', 'reduites'));
  await mockTauri(page);
  const errors = await openUi(page, '');
  await reunionPuisFin(page);
  await expect(page.locator('.rec')).toContainText('réunion terminée');
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'compose');
  await expect(page.locator('[data-card=compose] .text')).toHaveText('Je rédige le compte rendu…');
  await expect(page.locator('.rec')).toBeHidden({ timeout: 4000 });
  expect(await page.evaluate(() => document.querySelector('.kobi')!.getAnimations({ subtree: true }).filter((a) => a.playState === 'running').length)).toBe(0);
  await expectNoErrors(errors);
});

/**
 * Fin immédiate et annulable (KOB-245) : dès la fin, REC s'éteint, Kobi
 * rédige, et « Réunion terminée · Annuler » reste affiché pendant le délai.
 */
async function reunionPuisFinAnnulable(page: Page, debut: number): Promise<void> {
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: debut, notes: 3, titre: null, derniere_note: 'Budget validé' });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'write');
  await suivreEtats(page);
  await emit(page, 'state', { etat: 'fin', activite: null, mascotte: 'compose', transcrit: false });
  await emit(page, 'enregistrement', { ...ARRET, annulable: true });
}

test('fin annulable : REC éteint, Kobi rédige, « Annuler » reprend la même réunion (KOB-245)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const debut = Date.now() - 7 * 60_000;
  await reunionPuisFinAnnulable(page, debut);

  // Fin tout de suite : plus de REC, rédaction en cours, « Annuler » qui attend.
  const rec = page.locator('.rec');
  await expect(rec).toHaveClass(/is-ended/);
  await expect(rec).toContainText('réunion terminée');
  await expect(rec).not.toContainText('REC');
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'compose');
  const annuler = rec.getByRole('button', { name: 'Annuler' });
  await expect(annuler).toBeVisible();
  // « réunion terminée · Annuler », sans la durée : rien n'est tronqué.
  await expect(rec.locator('.rec-time')).toBeHidden();
  expect(await rec.locator('.rec-label').evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
  // L'étiquette ne s'efface pas tant que la fin s'annule.
  await page.waitForTimeout(2600);
  await expect(annuler).toBeVisible();

  await annuler.click();
  expect((await calls(page)).filter(([c]) => c === 'reprendre')).toHaveLength(1);
  // La coque reprend la même réunion : même début, le compteur continue.
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: debut, notes: 3, titre: null, derniere_note: 'Budget validé' });
  await expect(rec.locator('.rec-label')).toHaveText('REC');
  await expect(rec).not.toHaveClass(/is-ended|is-paused/);
  await expect(annuler).toBeHidden();
  await expect(rec.locator('.rec-time')).toHaveText(/^7:0\d$/);
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'write');
  await expectNoErrors(errors);
});

test('fin annulable : Ctrl+Z annule ; sans rien, « Annuler » disparaît à la fin du délai (KOB-245)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await reunionPuisFinAnnulable(page, Date.now() - 7 * 60_000);
  const rec = page.locator('.rec');
  await expect(rec.getByRole('button', { name: 'Annuler' })).toBeVisible();
  await page.keyboard.press('Control+z');
  expect((await calls(page)).filter(([c]) => c === 'reprendre')).toHaveLength(1);

  // Délai écoulé sans reprise : la coque annonce la fin définitive.
  await emit(page, 'enregistrement', ARRET);
  await expect(rec.getByRole('button', { name: 'Annuler' })).toBeHidden();
  await expect(rec).toBeHidden({ timeout: 4000 });
  // Ctrl+Z ne fait plus rien.
  await page.keyboard.press('Control+z');
  expect((await calls(page)).filter(([c]) => c === 'reprendre')).toHaveLength(1);
  await expect(page.locator('[data-card=compose] .text')).toHaveText('Je rédige le compte rendu…');
  await expectNoErrors(errors);
});

test('pause et fin distinctes : la pause montre « PAUSE », sans « Annuler » ni rédaction (KOB-245)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const debut = Date.now() - 7 * 60_000;
  await emit(page, 'state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true });
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: debut, notes: 3, titre: null });
  await emit(page, 'state', { etat: 'pause', activite: null, mascotte: 'idle', transcrit: false });
  await emit(page, 'enregistrement', { actif: true, pause: true, debut_ms: debut, notes: 3, titre: null });
  const rec = page.locator('.rec');
  await expect(rec).toHaveClass(/is-paused/);
  await expect(rec.locator('.rec-label')).toHaveText('PAUSE');
  await expect(rec.getByRole('button', { name: 'Annuler' })).toBeHidden();
  await expect(page.locator('[data-card=compose]')).toHaveCount(0);
  await expectNoErrors(errors);
});
