// La fenêtre de la mascotte reste transparente, quel que soit l'état de
// Kobi : jamais de fond derrière lui (règle du propriétaire). L'app Kobi,
// elle, garde un fond de fenêtre normal.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, mockTauri, openUi } from './helpers';

/** Couleur et image de fond de `html`, `body`, `#app` et de la scène. */
async function fonds(page: Page): Promise<Array<[string, string, string]>> {
  return page.evaluate(() =>
    ['html', 'body', '#app', '.stage']
      .map((s) => [s, document.querySelector(s)] as const)
      .filter(([, el]) => el)
      .map(([s, el]) => {
        const style = getComputedStyle(el!);
        return [s, style.backgroundColor, style.backgroundImage] as [string, string, string];
      }),
  );
}

async function attendreTransparent(page: Page): Promise<void> {
  const vus = await fonds(page);
  expect(vus.map(([s]) => s)).toEqual(expect.arrayContaining(['html', 'body', '.stage']));
  for (const [s, couleur, image] of vus) {
    expect(couleur, `fond de ${s}`).toBe('rgba(0, 0, 0, 0)');
    expect(image, `image de fond de ${s}`).toBe('none');
  }
  await expect(page.locator('.stage')).not.toHaveClass(/with-backdrop/);
}

test('la fenêtre de la mascotte est transparente, au repos comme en réunion', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await attendreTransparent(page);
  // Kobi écoute, puis une réunion est enregistrée avec sa fiche de notes.
  await page.evaluate(() => window.kobi.setState('listen'));
  await attendreTransparent(page);
  await page.evaluate(() =>
    window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 2, titre: 'Point hebdo', derniere_note: 'Budget validé' }),
  );
  await attendreTransparent(page);
  await expectNoErrors(errors);
});

test('l’app Kobi garde un fond de fenêtre normal', async ({ page }) => {
  await mockTauri(page, { carnet_index: { racine: '/home/test/Kobi', reunions: [], conversations: [], rappels: [] } });
  await page.goto('/app.html#notes');
  const fond = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(fond).not.toBe('rgba(0, 0, 0, 0)');
});
