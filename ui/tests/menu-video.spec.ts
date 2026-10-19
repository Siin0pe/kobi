import { test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

// Vidéo du menu du clic droit (KOB-139), jointe au ticket : fenêtre native
// (410 × 280) filmée en 820 × 560.
test.use({ viewport: { width: 410, height: 280 }, deviceScaleFactor: 2, video: { mode: 'on', size: { width: 820, height: 560 } } });
// Vidéo pour le ticket seulement, pas en vérification (KOB-172).
test.skip(!process.env.KOBI_PW_VIDEOS, 'vidéo des tickets : KOBI_PW_VIDEOS=1');

const menu = (page: Page) => page.getByRole('menu', { name: 'Menu de Kobi' });
const LABELS = ['Parler à Kobi', 'Écrire à Kobi', 'Enregistrer la réunion', 'Pause de l’écoute', 'Ouvrir Kobi', 'Ouvrir le Carnet', 'Réglages', 'Répondre à voix haute', 'Mode discret', 'Quitter Kobi'];

test('démonstration du menu (vidéo jointe au ticket)', async ({ page }) => {
  test.setTimeout(60_000);
  const errors = await openUi(page, '');
  await page.waitForTimeout(800);
  const box = (await page.locator('.kobi').boundingBox())!;
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x - 40, y - 20, { steps: 10 });
  await page.mouse.click(x, y, { button: 'right' });
  await page.waitForTimeout(600);
  for (const label of LABELS) {
    await menu(page).getByRole('menuitem', { name: label }).hover();
    await page.waitForTimeout(260);
  }
  await menu(page).getByRole('menuitem', { name: 'Enregistrer la réunion' }).click();
  await page.waitForTimeout(1200);
  await page.keyboard.press('Shift+F10');
  await page.waitForTimeout(500);
  await page.keyboard.press('ArrowDown');
  await page.waitForTimeout(250);
  await page.keyboard.press('ArrowDown');
  await page.waitForTimeout(250);
  await page.keyboard.press('ArrowDown');
  await page.waitForTimeout(400);
  await page.keyboard.press('Enter'); // pause de l'enregistrement
  await page.waitForTimeout(1000);
  await page.mouse.click(x, y, { button: 'right' });
  await page.waitForTimeout(500);
  await menu(page).getByRole('menuitem', { name: 'Quitter Kobi' }).click();
  await page.waitForTimeout(1200);
  await page.keyboard.press('Escape');
  await page.waitForTimeout(400);
  await page.mouse.click(x, y, { button: 'right' });
  await page.waitForTimeout(400);
  await menu(page).getByRole('menuitem', { name: 'Mode discret' }).click();
  await page.waitForTimeout(900);
  await page.locator('.dot').click({ button: 'right', force: true });
  await page.waitForTimeout(500);
  await menu(page).getByRole('menuitem', { name: 'Déplier Kobi' }).click();
  await page.waitForTimeout(900);
  await expectNoErrors(errors);
});
