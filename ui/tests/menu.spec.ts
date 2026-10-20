import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi, sansAnimation } from './helpers';

// Menu du clic droit (KOB-139), à la taille de la fenêtre native (410 × 280).
test.use({ viewport: { width: 410, height: 280 } });

const LABELS = ['Parler à Kobi', 'Écrire à Kobi', 'Enregistrer la réunion', 'Pause de l’écoute', 'Ouvrir Kobi', 'Ouvrir le Carnet', 'Réglages', 'Répondre à voix haute', 'Mode discret', 'Quitter Kobi'];

async function rightClickKobi(page: Page): Promise<void> {
  const box = (await page.locator('.kobi').boundingBox())!;
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' });
}

function menu(page: Page) {
  return page.getByRole('menu', { name: 'Menu de Kobi' });
}

async function labels(page: Page): Promise<string[]> {
  return menu(page).getByRole('menuitem').evaluateAll((items) => items.map((i) => i.querySelector('.kmenu-label')!.textContent!));
}

test('clic droit sur Kobi : le menu de Kobi, pas celui du navigateur', async ({ page }) => {
  const errors = await openUi(page, '');
  const prevented = page.evaluate(
    () =>
      new Promise<boolean>((resolve) =>
        document.addEventListener('contextmenu', (e) => setTimeout(() => resolve(e.defaultPrevented)), { once: true }),
      ),
  );
  await rightClickKobi(page);
  expect(await prevented, 'menu natif empêché').toBe(true);
  await expect(menu(page)).toBeVisible();
  expect(await labels(page)).toEqual(LABELS);
  await expect(menu(page).getByRole('menuitem').first()).toBeFocused();
  // Le menu tient dans la fenêtre, une fois posé.
  await sansAnimation(menu(page), true);
  const box = (await menu(page).boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(410);
  expect(box.y + box.height).toBeLessThanOrEqual(280);
  await page.screenshot({ path: test.info().outputPath('menu.png') });

  // Un clic ailleurs le ferme.
  await page.mouse.click(5, 5);
  await expect(menu(page)).toBeHidden();
  await expectNoErrors(errors);
});

test('clic droit près du bord : le menu reste dans la fenêtre', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.mouse.click(405, 275, { button: 'right' });
  await sansAnimation(menu(page), true);
  const box = (await menu(page).boundingBox())!;
  expect(box.x + box.width).toBeLessThanOrEqual(410);
  expect(box.y + box.height).toBeLessThanOrEqual(280);
  await expectNoErrors(errors);
});

// KOB-217 : le texte des bulles se sélectionne (et se copie), pas Kobi
// lui-même. Ce test exigeait l'inverse pour la bulle (KOB-139) : il échouait
// dès que le glisser tombait sur le texte de la réponse (KOB-195).
test('Kobi ne se sélectionne pas ; le texte de la bulle et le champ de saisie si', async ({ page }) => {
  const errors = await openUi(page, 'state=answer');
  const card = page.locator('[data-card=answer]');
  await expect(card).toBeVisible();
  // Glisser sur la tête de Kobi : rien n'est sélectionné.
  const tete = (await page.locator('.kobi .pill').boundingBox())!;
  await page.mouse.move(tete.x + 20, tete.y + tete.height / 2);
  await page.mouse.down();
  await page.mouse.move(tete.x + tete.width - 20, tete.y + tete.height / 2, { steps: 8 });
  await page.mouse.up();
  expect(await page.evaluate(() => getSelection()?.toString() ?? '')).toBe('');
  expect(await page.locator('.kobi .pill').evaluate((n) => getComputedStyle(n).userSelect)).toBe('none');
  // Le texte de la réponse, lui, se sélectionne.
  const texte = card.locator('.text');
  await expect(texte).toHaveCSS('user-select', 'text');
  const t = (await texte.boundingBox())!;
  await page.mouse.move(t.x + 2, t.y + 8);
  await page.mouse.down();
  await page.mouse.move(t.x + t.width - 4, t.y + 8, { steps: 8 });
  await page.mouse.up();
  expect((await page.evaluate(() => getSelection()?.toString() ?? '')).length).toBeGreaterThan(3);
  await page.evaluate(() => getSelection()?.removeAllRanges());

  // La bulle d'écriture (KOB-147), ouverte par le menu, garde sélection et menu natif.
  await rightClickKobi(page);
  await menu(page).getByRole('menuitem', { name: 'Écrire à Kobi' }).click();
  const input = page.getByRole('textbox', { name: 'Écrire à Kobi' });
  await expect(input).toBeFocused();
  await input.fill('texte à copier');
  await input.selectText();
  expect(await input.evaluate((i: HTMLInputElement) => i.value.slice(i.selectionStart!, i.selectionEnd!))).toBe('texte à copier');
  await input.click({ button: 'right' });
  await expect(menu(page)).toBeHidden();
  await expectNoErrors(errors);
});

test('au clavier : Maj+F10, flèches, Échap, Entrée', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.keyboard.press('Shift+F10');
  await expect(menu(page)).toBeVisible();
  const items = menu(page).getByRole('menuitem');
  await expect(items.nth(0)).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(items.nth(1)).toBeFocused();
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowUp');
  await expect(items.nth(9)).toBeFocused();
  await page.keyboard.press('Home');
  await expect(items.nth(0)).toBeFocused();
  await page.keyboard.press('r'); // Réglages
  await expect(items.nth(6)).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menu(page)).toBeHidden();

  // Échap ferme le menu sans couper la réponse en cours.
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  await page.keyboard.press('Shift+F10');
  await page.keyboard.press('Escape');
  await expect(menu(page)).toBeHidden();
  await expect(page.locator('.kobi')).toHaveAttribute('data-state', 'answer');
  await page.evaluate(() => window.kobi.setState('idle'));

  // Touche Menu, puis Entrée sur « Enregistrer la réunion ».
  await page.keyboard.press('ContextMenu');
  await expect(menu(page)).toBeVisible();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(menu(page)).toBeHidden();
  await expect(page.locator('.rec')).toBeVisible();
  await page.keyboard.press('Shift+F10');
  expect((await labels(page)).slice(2, 4)).toEqual(['Arrêter l’enregistrement', 'Pause de l’enregistrement']);
  await expectNoErrors(errors);
});

test('chaque entrée fait son effet (hors de l’application)', async ({ page }) => {
  const errors = await openUi(page, '');
  const choose = async (label: string): Promise<void> => {
    await rightClickKobi(page);
    await menu(page).getByRole('menuitem', { name: label }).click();
    await expect(menu(page)).toBeHidden();
  };

  await choose('Parler à Kobi');
  await expect(page.locator('.kobi')).toHaveAttribute('data-state', 'listen');
  await page.evaluate(() => window.kobi.setState('idle'));

  // Pause de l'écoute hors réunion : « micro coupé » visible.
  await choose('Pause de l’écoute');
  await expect(page.getByText('micro coupé')).toBeVisible();
  // Ni la pastille ni le message ne couvrent la bulle d'écriture (KOB-147).
  await choose('Écrire à Kobi');
  await expect(page.getByRole('textbox', { name: 'Écrire à Kobi' })).toBeFocused();
  const field = (await page.getByRole('textbox', { name: 'Écrire à Kobi' }).boundingBox())!;
  const pill = (await page.locator('.kmenu-micoff').boundingBox())!;
  expect(pill.y + pill.height <= field.y || pill.x >= field.x + field.width).toBe(true);
  await rightClickKobi(page);
  await expect(menu(page).getByRole('menuitem', { name: 'Reprendre l’écoute' })).toBeVisible();
  await page.keyboard.press('Escape');
  await choose('Reprendre l’écoute');
  await expect(page.getByText('micro coupé')).toBeHidden();

  // Réunion : enregistrer, pause, reprise, arrêt.
  await choose('Enregistrer la réunion');
  await expect(page.locator('.rec')).toBeVisible();
  await choose('Pause de l’enregistrement');
  expect(await page.evaluate(() => window.kobi.stage.currentMeeting?.paused)).toBe(true);
  await choose('Reprendre l’enregistrement');
  expect(await page.evaluate(() => window.kobi.stage.currentMeeting?.paused)).toBe(false);
  await choose('Arrêter l’enregistrement');
  await expect(page.locator('.rec')).toBeHidden();

  // Carnet et réglages : dits indisponibles hors de l'application.
  await choose('Ouvrir le Carnet');
  await expect(page.locator('.kmenu-toast').filter({ hasText: 'Carnet' })).toBeVisible();
  const toast = (await page.locator('.kmenu-toast').boundingBox())!;
  expect(toast.y + toast.height).toBeLessThanOrEqual(field.y);
  await choose('Réglages');
  await expect(page.locator('.kmenu-toast').filter({ hasText: 'réglages' })).toBeVisible();

  // Mode discret, puis clic droit sur la pastille pour déplier.
  await choose('Mode discret');
  await expect(page.locator('.dot')).toBeVisible();
  await page.locator('.dot').click({ button: 'right', force: true });
  await menu(page).getByRole('menuitem', { name: 'Déplier Kobi' }).click();
  await expect(page.locator('.scene')).toBeVisible();

  // Quitter sans réunion : tout de suite.
  const quit = page.evaluate(() => new Promise<boolean>((r) => addEventListener('kobi:quitter', () => r(true), { once: true })));
  await choose('Quitter Kobi');
  expect(await quit).toBe(true);
  await expectNoErrors(errors);
});

test('quitter pendant une réunion demande confirmation', async ({ page }) => {
  const errors = await openUi(page, 'reunion=Point&carte=0');
  let quits = 0;
  await page.exposeFunction('quitte', () => quits++);
  await page.evaluate(() => addEventListener('kobi:quitter', () => (window as unknown as { quitte(): void }).quitte()));
  await rightClickKobi(page);
  await menu(page).getByRole('menuitem', { name: 'Quitter Kobi' }).click();
  const dialog = page.getByRole('alertdialog', { name: 'Une réunion est en cours.' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Annuler' })).toBeFocused();
  await sansAnimation(dialog);
  // Centrée dans la fenêtre, entière.
  const box = (await dialog.boundingBox())!;
  expect(Math.abs(box.x + box.width / 2 - 205)).toBeLessThan(3);
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(410);
  await page.screenshot({ path: test.info().outputPath('confirmation.png') });
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  expect(quits).toBe(0);
  await expect(page.locator('.rec')).toBeVisible();

  await rightClickKobi(page);
  await menu(page).getByRole('menuitem', { name: 'Quitter Kobi' }).click();
  await dialog.getByRole('button', { name: 'Arrêter et quitter' }).click();
  await expect.poll(() => quits).toBe(1);
  await expect(page.locator('.rec')).toBeHidden();
  await expectNoErrors(errors);
});

/**
 * Coque simulée : `menu_kobi` renvoie `kobi://menu` comme `menu.rs` ; les
 * entrées de `refus` sont refusées avec ce message, comme une erreur Rust.
 */
async function fakeShell(page: Page, refus: Record<string, string> = {}): Promise<void> {
  await page.addInitScript((refus) => {
    const calls: Array<{ cmd: string; args?: unknown }> = [];
    let voix = false;
    const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
    const emit = (event: string, payload: unknown): void => (listeners[event] ?? []).forEach((h) => h({ payload }));
    (window as unknown as Record<string, unknown>).__calls = calls;
    (window as unknown as Record<string, unknown>).__emit = emit;
    (window as unknown as Record<string, unknown>).__TAURI__ = {
      window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
      event: {
        listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
          (listeners[event] ??= []).push(handler);
          return () => {};
        },
      },
      core: {
        invoke: async (cmd: string, args?: { entree?: string }) => {
          calls.push({ cmd, args });
          if (cmd === 'etat_session') return { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false };
          if (cmd === 'etat_ecoute') return false;
          if (cmd === 'menu_kobi') {
            if (args!.entree! in refus) throw refus[args!.entree!];
            emit('kobi://menu', args!.entree);
            if (args!.entree === 'pause') emit('ecoute', true);
            if (args!.entree === 'voix') emit('voix', { actif: (voix = !voix), parle: false });
            return null;
          }
          throw `Command ${cmd} not found`;
        },
      },
    };
  }, refus);
}

test('dans l’application : chaque entrée passe par la coque, un refus est dit par Kobi', async ({ page }) => {
  await fakeShell(page, { reglages: 'Réglages : fenêtre impossible' });
  const errors = await openUi(page, '');
  const calls = (): Promise<Array<{ cmd: string; args?: { entree?: string } }>> =>
    page.evaluate(() => (window as unknown as { __calls: Array<{ cmd: string; args?: { entree?: string } }> }).__calls);
  for (const label of ['Ouvrir Kobi', 'Ouvrir le Carnet', 'Réglages', 'Pause de l’écoute', 'Mode discret']) {
    await rightClickKobi(page);
    await menu(page).getByRole('menuitem', { name: label }).click();
    if (label === 'Réglages') await expect(page.locator('.kmenu-toast')).toHaveText('Réglages : fenêtre impossible');
  }
  await expect(page.getByText('micro coupé')).toBeHidden(); // masqué en mode discret
  await expect(page.locator('.dot')).toBeVisible();
  // L'app Kobi (Kobi, Carnet, Réglages) est ouverte par la coque, pas par l'interface.
  // (`ouvrir_menu` : la fenêtre du menu, KOB-202, refusée par cette coque simulée.)
  const cmds = (await calls()).filter((c) => c.cmd === 'menu_kobi' || (c.cmd.startsWith('ouvrir_') && c.cmd !== 'ouvrir_menu'));
  expect(cmds).toEqual([
    { cmd: 'menu_kobi', args: { entree: 'app' } },
    { cmd: 'menu_kobi', args: { entree: 'carnet' } },
    { cmd: 'menu_kobi', args: { entree: 'reglages' } },
    { cmd: 'menu_kobi', args: { entree: 'pause' } },
    { cmd: 'menu_kobi', args: { entree: 'discret' } },
  ]);
  // Une entrée venue de la barre système (événement seul) a le même effet.
  await page.evaluate(() => (window as unknown as { __emit(e: string, p: unknown): void }).__emit('kobi://menu', 'discret'));
  await expect(page.locator('.scene')).toBeVisible();
  await expect(page.getByText('micro coupé')).toBeVisible();
  await expectNoErrors(errors);
});

test('réduire les animations : le menu apparaît sans mouvement', async ({ page }) => {
  const errors = await openUi(page, 'reduced=1');
  await rightClickKobi(page);
  await expect(menu(page)).toBeVisible();
  const running = await menu(page).evaluate((m) => m.getAnimations({ subtree: true }).length);
  expect(running).toBe(0);
  await expectNoErrors(errors);
});

test('réponse à voix haute ou en texte : basculée par la coque, libellé qui suit (KOB-142)', async ({ page }) => {
  await fakeShell(page);
  const errors = await openUi(page, '');
  await rightClickKobi(page);
  await menu(page).getByRole('menuitem', { name: 'Répondre à voix haute' }).click();
  await expect(page.locator('.kmenu-toast')).toHaveText('Je réponds à voix haute.');
  await expect(page.locator('.stage')).toHaveAttribute('data-voix', 'active');
  await rightClickKobi(page);
  await menu(page).getByRole('menuitem', { name: 'Répondre en texte seul' }).click();
  await expect(page.locator('.kmenu-toast')).toHaveText('Je réponds par écrit.');
  await expect(page.locator('.stage')).toHaveAttribute('data-voix', 'texte');
  const cmds = await page.evaluate(() => (window as unknown as { __calls: Array<{ cmd: string; args?: { entree?: string } }> }).__calls.filter((c) => c.cmd === 'menu_kobi').map((c) => c.args!.entree));
  expect(cmds).toEqual(['voix', 'voix']);
  await expectNoErrors(errors);
});

test('fenêtre de la mascotte transparente, menu ouvert ou non ; une seule chose à la fois autour de Kobi', async ({ page }) => {
  const errors = await openUi(page, '');
  const fonds = () =>
    page.evaluate(() =>
      [document.documentElement, document.body, document.querySelector('.stage')!, document.querySelector('.scene')!].map((n) => getComputedStyle(n).backgroundColor),
    );
  const transparent = ['rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)'];
  expect(await fonds()).toEqual(transparent);
  await rightClickKobi(page);
  await expect(menu(page)).toBeVisible();
  expect(await fonds()).toEqual(transparent);
  // Micro coupé, puis un message : le message passe devant, la pastille revient après.
  await menu(page).getByRole('menuitem', { name: 'Pause de l’écoute' }).click();
  await expect(page.getByText('micro coupé')).toBeVisible();
  await rightClickKobi(page);
  await menu(page).getByRole('menuitem', { name: 'Ouvrir le Carnet' }).click();
  await expect(page.locator('.kmenu-toast')).toBeVisible();
  await expect(page.getByText('micro coupé')).toBeHidden();
  await expect(page.locator('.kmenu-toast')).toBeHidden({ timeout: 4000 });
  await expect(page.getByText('micro coupé')).toBeVisible();
  expect(await fonds()).toEqual(transparent);
  await expectNoErrors(errors);
});
