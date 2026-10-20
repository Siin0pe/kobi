// Interactions propres (KOB-217) : on ne déplace Kobi que par sa tête, le
// texte des réponses et des cartes se sélectionne sans rien déplacer ni
// ouvrir, et la coque reçoit les zones actives (le reste de la fenêtre
// transparente laisse passer la souris).
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

const REPONSE = 'Le verrou protège la donnée partagée. Chaque fil attend son tour avant de lire ou d’écrire, puis le relâche à la fin du bloc.';

/** Kobi dans une coque simulée qui compte les déplacements de fenêtre. */
async function kobi(page: Page): Promise<string[]> {
  await mockTauri(page);
  await page.addInitScript(() => {
    localStorage.setItem('kobi.affichage', JSON.stringify({ v: 2, mode: 'detaille', auto: 'jamais' }));
    const w = window as unknown as { __TAURI__: { window: unknown }; __deplacements: number };
    w.__deplacements = 0;
    w.__TAURI__.window = { getCurrentWindow: () => ({ startDragging: async () => void w.__deplacements++ }) };
  });
  return openUi(page, '');
}

const deplacements = (page: Page) => page.evaluate(() => (window as unknown as { __deplacements: number }).__deplacements);

/** Appui, glisser de 60 px, relâchement. */
async function glisser(page: Page, x: number, y: number): Promise<void> {
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 60, y + 10, { steps: 8 });
  await page.mouse.up();
}

async function repondre(page: Page): Promise<void> {
  await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'answer', REPONSE);
  await expect(page.locator('[data-card=answer]')).toHaveAttribute('data-typing', '0', { timeout: 15_000 });
}

test('on ne déplace Kobi que par sa tête ; ailleurs, un glisser ne déplace rien', async ({ page }) => {
  const errors = await kobi(page);
  // Dans la scène, à côté de Kobi (corps, vide) : rien ne bouge.
  const scene = (await page.locator('.kobi').boundingBox())!;
  await glisser(page, scene.x + 12, scene.y + scene.height - 12);
  await glisser(page, 20, 20);
  expect(await deplacements(page)).toBe(0);
  // Par la tête : la fenêtre se déplace, curseur « déplacer » au survol.
  const tete = page.locator('.kobi .pill');
  await expect(tete).toHaveCSS('cursor', 'move');
  const b = (await tete.boundingBox())!;
  await glisser(page, b.x + b.width / 2, b.y + b.height / 2);
  expect(await deplacements(page)).toBe(1);
  await expectNoErrors(errors);
});

test('le texte de la bulle se sélectionne et se copie, sans déplacer Kobi ni ouvrir la vue', async ({ page }) => {
  const errors = await kobi(page);
  await repondre(page);
  const texte = page.locator('[data-card=answer] .text');
  await expect(texte).toHaveCSS('user-select', 'text');
  const b = (await texte.boundingBox())!;
  await page.mouse.move(b.x + 4, b.y + 8);
  await page.mouse.down();
  await page.mouse.move(b.x + b.width - 8, b.y + 8, { steps: 10 });
  await page.mouse.up();
  const choisi = await page.evaluate(() => window.getSelection()?.toString() ?? '');
  expect(choisi.length).toBeGreaterThan(10);
  expect(REPONSE).toContain(choisi.trim().slice(0, 10));
  await page.waitForTimeout(400);
  await expect(page.getByRole('dialog', { name: 'Conversation avec Kobi' })).toBeHidden();
  expect(await deplacements(page)).toBe(0);
  // Un simple clic sur la bulle l'ouvre toujours en grand.
  await page.evaluate(() => window.getSelection()?.removeAllRanges());
  // La bulle vit (elle flotte) : un clic par coordonnées, sans attendre qu'elle s'immobilise.
  await page.mouse.click(b.x + b.width / 2, b.y + 8);
  await expect(page.getByRole('dialog', { name: 'Conversation avec Kobi' })).toBeVisible();
  await expectNoErrors(errors);
});

test('la coque reçoit les zones actives (Kobi et la bulle) et l’état du bouton', async ({ page }) => {
  const errors = await kobi(page);
  const zones = async () =>
    (await calls(page)).filter(([c]) => c === 'zones_actives').map(([, a]) => (a as { zones: number[][] }).zones).pop() ?? [];
  const dedans = (z: number[][], x: number, y: number) => z.some(([zx, zy, l, h]) => x >= zx! && x < zx! + l! && y >= zy! && y < zy! + h!);
  await expect.poll(async () => (await zones()).length).toBeGreaterThan(0);
  const tete = (await page.locator('.kobi .pill').boundingBox())!;
  expect(dedans(await zones(), tete.x + tete.width / 2, tete.y + tete.height / 2)).toBe(true);
  // Un coin vide de la fenêtre n'est pas une zone : la souris y traverse.
  expect(dedans(await zones(), 2, 2)).toBe(false);
  // La bulle qui apparaît devient une zone.
  await repondre(page);
  const bulle = (await page.locator('[data-card=answer]').boundingBox())!;
  await expect.poll(async () => dedans(await zones(), bulle.x + 10, bulle.y + 10)).toBe(true);
  // Appui puis relâchement : la coque le sait (pas de traversée pendant un appui).
  await page.mouse.move(bulle.x + 20, bulle.y + 20);
  await page.mouse.down();
  await page.mouse.up();
  const appuis = (await calls(page)).filter(([c]) => c === 'appui_souris').map(([, a]) => (a as { appui: boolean }).appui);
  expect(appuis.slice(-2)).toEqual([true, false]);
  await expectNoErrors(errors);
});
