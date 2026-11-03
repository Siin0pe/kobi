// Barres de défilement de l'app (retour du 26/10) : la liste des panneaux
// des Réglages défile seule, à la molette et à la barre ; les barres ont le
// style de l'app. Chromium sans fenêtre masque les barres : on les rend,
// comme WebView2.
import { expect, test, type Page } from '@playwright/test';
import { fakeTauri } from './app-fausse-coque';
import { expectNoErrors, sansAnimation } from './helpers';

test.use({ launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] } });

async function ouvrirApp(page: Page, ancre: string): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.goto(`/app.html${ancre}`);
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

test('Réglages : la liste des panneaux défile seule dans une petite fenêtre, à la molette et à la barre', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 420 });
  const errors = await ouvrirApp(page, '#reglages');
  const cote = page.locator('.vue-reglages .cote');
  await expect(cote).toBeVisible();
  // La vue arrive en glissant de 8 px, avec un ressort qui dépasse : mesurée
  // en route, la colonne déborde d'une fraction de pixel (KOB-404).
  await sansAnimation(page.locator('.vue-reglages'));
  const mesure = () => cote.evaluate((c) => ({ haut: c.scrollTop, plein: c.scrollHeight, vu: c.clientHeight, bas: c.getBoundingClientRect().bottom }));
  const avant = await mesure();
  expect(avant.plein, 'la liste dépasse la fenêtre').toBeGreaterThan(avant.vu);
  expect(avant.bas, 'la colonne tient dans la fenêtre').toBeLessThanOrEqual(422);
  // Molette sur la liste.
  const b = (await cote.boundingBox())!;
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2);
  await page.mouse.wheel(0, 400);
  await expect.poll(async () => (await mesure()).haut).toBeGreaterThan(0);
  // Barre de défilement : la pilule se tire vers le bas.
  await cote.evaluate((c) => (c.scrollTop = 0));
  await page.mouse.move(b.x + b.width - 6, b.y + 30);
  await page.mouse.down();
  await page.mouse.move(b.x + b.width - 6, b.y + b.height - 10, { steps: 8 });
  await page.mouse.up();
  await expect.poll(async () => (await mesure()).haut).toBeGreaterThan(0);
  // La dernière entrée s'atteint.
  await cote.evaluate((c) => (c.scrollTop = c.scrollHeight));
  await expect(page.locator('.vue-reglages .menu-entree').last()).toBeInViewport();
  await page.screenshot({ path: test.info().outputPath('reglages-defilement.png') });
  await expectNoErrors(errors);
});
