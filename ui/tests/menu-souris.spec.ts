// Menu du clic droit sous la souris (KOB-202) : la page du menu (menu.html)
// se place sous le pointeur, en pixels physiques, et se retourne vers la
// gauche ou le haut aux quatre coins de la zone de travail ; elle se ferme
// comme un menu natif. Côté mascotte, le clic droit demande la fenêtre du
// menu à la coque et exécute l'entrée qu'elle renvoie. La pose de la
// fenêtre elle-même est testée côté Rust (crates/kobi-app/src/menu_souris.rs)
// et sous Xvfb.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

interface Appel {
  cmd: string;
  args?: Record<string, unknown>;
}

/** Coque simulée de la fenêtre du menu : elle note les commandes. */
async function pageMenu(page: Page): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(() => {
    const appels: Appel[] = [];
    (window as unknown as Record<string, unknown>).__appels = appels;
    (window as unknown as Record<string, unknown>).__TAURI__ = {
      event: { listen: async () => () => {} },
      core: {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          appels.push({ cmd, args });
          return null;
        },
      },
    };
  });
  await page.goto('/menu.html');
  await page.waitForFunction(() => '__ouvrirMenu' in window);
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

const appels = (page: Page): Promise<Appel[]> => page.evaluate(() => (window as unknown as { __appels: Appel[] }).__appels);

const CONTEXTE = { recording: false, paused: false, compact: false, voice: false };
/** Écran de 1920 × 1080 à 150 %, barre des tâches de 48 px en bas. */
const ZONE = { x: 0, y: 0, largeur: 1920, hauteur: 1032 };
const ECHELLE = 1.5;
/** Place de la carte dans la fenêtre (menu-fenetre.ts) : liseré à gauche et en haut. */
const BORD = 3;

async function ouvrir(page: Page, x: number, y: number, zone = ZONE, echelle = ECHELLE): Promise<{ x: number; y: number; largeur: number; hauteur: number; coin: string; carte: { largeur: number; hauteur: number } }> {
  await page.evaluate(
    ({ x, y, zone, echelle, contexte }) => (window as unknown as { __ouvrirMenu(o: unknown): Promise<void> }).__ouvrirMenu({ pointeur: { x, y }, zone, echelle, contexte }),
    { x, y, zone, echelle, contexte: CONTEXTE },
  );
  const montre = (await appels(page)).filter((a) => a.cmd === 'montrer_menu').at(-1)!.args as { x: number; y: number; largeur: number; hauteur: number };
  const coin = (await page.locator('.kmenu').getAttribute('data-coin'))!;
  // Taille de mise en page : pendant l'éventail d'ouverture, la boîte affichée est plus petite.
  const [w, h] = await page.locator('.kmenu').evaluate((el) => [(el as HTMLElement).offsetWidth, (el as HTMLElement).offsetHeight]);
  return { ...montre, coin, carte: { largeur: w * echelle, hauteur: h * echelle } };
}

/** Coin haut-gauche de la carte sur l'écran, en pixels physiques. */
const carte = (m: { x: number; y: number }, echelle = ECHELLE): { x: number; y: number } => ({ x: m.x + BORD * echelle, y: m.y + BORD * echelle });

test('aux quatre coins de l’écran : sous le pointeur, retourné s’il déborde, toujours entier', async ({ page }) => {
  const errors = await pageMenu(page);
  const cas = [
    { nom: 'haut-gauche', x: 40, y: 30, coin: 'haut-gauche' },
    { nom: 'haut-droite', x: 1900, y: 30, coin: 'haut-droite' },
    { nom: 'bas-gauche', x: 40, y: 1020, coin: 'bas-gauche' },
    { nom: 'bas-droite', x: 1900, y: 1020, coin: 'bas-droite' },
  ];
  for (const c of cas) {
    const m = await ouvrir(page, c.x, c.y);
    const p = carte(m);
    const { largeur, hauteur } = m.carte;
    expect(m.coin, c.nom).toBe(c.coin);
    // Le coin du menu qui touche le pointeur est celui annoncé.
    const versGauche = c.coin.endsWith('droite');
    const versHaut = c.coin.startsWith('bas');
    expect(Math.abs((versGauche ? p.x + largeur : p.x) - c.x), `${c.nom} x`).toBeLessThan(2);
    expect(Math.abs((versHaut ? p.y + hauteur : p.y) - c.y), `${c.nom} y`).toBeLessThan(2);
    // Entier dans la zone de travail (barre des tâches comprise).
    expect(p.x, c.nom).toBeGreaterThanOrEqual(ZONE.x);
    expect(p.y, c.nom).toBeGreaterThanOrEqual(ZONE.y);
    expect(p.x + largeur, c.nom).toBeLessThanOrEqual(ZONE.x + ZONE.largeur + 1);
    expect(p.y + hauteur, c.nom).toBeLessThanOrEqual(ZONE.y + ZONE.hauteur + 1);
    // L'éventail part du coin sous le pointeur.
    const origine = await page.locator('.kmenu').evaluate((el) => [el.style.getPropertyValue('--ox'), el.style.getPropertyValue('--oy')]);
    expect(origine[0] !== '0px', c.nom).toBe(versGauche);
    expect(origine[1] !== '0px', c.nom).toBe(versHaut);
  }
  await expectNoErrors(errors);
});

test('second écran à 100 % à droite du premier, pointeur sur la barre des tâches : le menu reste dans la zone', async ({ page }) => {
  const errors = await pageMenu(page);
  const zone = { x: 1920, y: 0, largeur: 1280, hauteur: 984 };
  const m = await ouvrir(page, 2000, 1010, zone, 1);
  const p = carte(m, 1);
  expect(m.coin).toBe('bas-gauche');
  expect(p.x).toBeGreaterThanOrEqual(1920);
  expect(p.y + m.carte.hauteur).toBeLessThanOrEqual(984 + 1);
  // La fenêtre prend la taille de la carte, liseré et ombre comprises.
  expect(m.largeur).toBeGreaterThan(m.carte.largeur);
  expect(m.hauteur).toBeGreaterThan(m.carte.hauteur);
  await expectNoErrors(errors);
});

test('fermeture comme un menu natif : choix, Échap, clavier, perte du focus', async ({ page }) => {
  const errors = await pageMenu(page);
  const fermetures = async (): Promise<unknown[]> => (await appels(page)).filter((a) => a.cmd === 'menu_choisi').map((a) => a.args?.entree);
  await ouvrir(page, 200, 200);
  await expect(page.getByRole('menuitem').first()).toBeFocused();
  await page.getByRole('menuitem', { name: 'Ouvrir le Carnet' }).click();
  await expect.poll(fermetures).toEqual(['carnet']);
  await ouvrir(page, 200, 200);
  await page.keyboard.press('Escape');
  await expect.poll(fermetures).toEqual(['carnet', null]);
  await ouvrir(page, 200, 200);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect.poll(fermetures).toEqual(['carnet', null, 'ecrire']);
  await ouvrir(page, 200, 200);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await expect.poll(fermetures).toEqual(['carnet', null, 'ecrire', null]);
  await expect(page.locator('.kmenu')).toBeHidden();
  await expectNoErrors(errors);
});

test('fenêtre du menu refusée par la coque : le menu se referme et la mascotte est prévenue', async ({ page }) => {
  await page.addInitScript(() => {
    const appels: Appel[] = [];
    (window as unknown as Record<string, unknown>).__appels = appels;
    (window as unknown as Record<string, unknown>).__TAURI__ = {
      event: { listen: async () => () => {} },
      core: {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          appels.push({ cmd, args });
          if (cmd === 'montrer_menu') throw 'fenêtre du menu indisponible';
          return null;
        },
      },
    };
  });
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto('/menu.html');
  await page.waitForFunction(() => '__ouvrirMenu' in window);
  await page.evaluate(() =>
    (window as unknown as { __ouvrirMenu(o: unknown): Promise<void> }).__ouvrirMenu({
      pointeur: { x: 100, y: 100 },
      zone: { x: 0, y: 0, largeur: 1920, hauteur: 1032 },
      echelle: 1,
      contexte: { recording: false, paused: false, compact: false, voice: false, ending: false },
    }),
  );
  await expect(page.locator('.kmenu')).toBeHidden();
  expect((await appels(page)).filter((a) => a.cmd === 'menu_choisi').map((a) => a.args?.entree)).toEqual([null]);
  await expectNoErrors(errors);
});

test('libellés selon l’état : réunion en cours', async ({ page }) => {
  const errors = await pageMenu(page);
  await page.evaluate(() =>
    (window as unknown as { __ouvrirMenu(o: unknown): Promise<void> }).__ouvrirMenu({
      pointeur: { x: 100, y: 100 },
      zone: { x: 0, y: 0, largeur: 1920, hauteur: 1032 },
      echelle: 1,
      contexte: { recording: true, paused: false, compact: true, voice: true },
    }),
  );
  await expect(page.getByRole('menuitem', { name: 'Arrêter l’enregistrement' })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Déplier Kobi' })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Répondre en texte seul' })).toBeVisible();
  await expect(page.locator('.kmenu-state')).toHaveText('● réunion en cours');
  // Fenêtre transparente : seule la carte du menu se voit.
  const fonds = await page.evaluate(() => [getComputedStyle(document.documentElement).backgroundColor, getComputedStyle(document.body).backgroundColor]);
  expect(fonds).toEqual(['rgba(0, 0, 0, 0)', 'rgba(0, 0, 0, 0)']);
  await expectNoErrors(errors);
});

test('mascotte : le clic droit demande la fenêtre du menu, puis exécute l’entrée choisie', async ({ page }) => {
  await page.addInitScript(() => {
    const appels: Appel[] = [];
    const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
    (window as unknown as Record<string, unknown>).__appels = appels;
    (window as unknown as Record<string, unknown>).__emit = (event: string, payload: unknown): void => (listeners[event] ?? []).forEach((h) => h({ payload }));
    (window as unknown as Record<string, unknown>).__TAURI__ = {
      window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
      event: {
        listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
          (listeners[event] ??= []).push(handler);
          return () => {};
        },
      },
      core: {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          appels.push({ cmd, args });
          if (cmd === 'etat_session') return { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false };
          if (cmd === 'etat_ecoute') return false;
          if (cmd === 'ouvrir_menu' || cmd === 'menu_kobi') return null;
          throw `Command ${cmd} not found`;
        },
      },
    };
  });
  const errors = await openUi(page, '');
  const box = (await page.locator('.kobi').boundingBox())!;
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' });
  await expect.poll(async () => (await appels(page)).filter((a) => a.cmd === 'ouvrir_menu').length).toBe(1);
  const ouverture = (await appels(page)).find((a) => a.cmd === 'ouvrir_menu')!;
  expect(ouverture.args).toEqual({ contexte: { recording: false, paused: false, compact: false, voice: false, ending: false }, clavier: false });
  // Le menu n'est pas dessiné dans la fenêtre de Kobi : la mascotte ne bouge pas.
  await expect(page.locator('.stage > .kmenu')).toBeHidden();
  await expect(page.locator('.stage')).toHaveClass(/menu-open/);
  // Maj+F10 : ouvert sur Kobi.
  await page.keyboard.press('Shift+F10');
  await expect.poll(async () => (await appels(page)).filter((a) => a.cmd === 'ouvrir_menu').at(-1)?.args?.clavier).toBe(true);
  // L'entrée choisie dans la fenêtre du menu revient ici et part à la coque.
  await page.evaluate(() => {
    const emit = (window as unknown as { __emit(e: string, p: unknown): void }).__emit;
    emit('menu://choix', 'carnet');
    emit('menu://ferme', null);
  });
  await expect.poll(async () => (await appels(page)).filter((a) => a.cmd === 'menu_kobi').map((a) => a.args?.entree)).toEqual(['carnet']);
  await expect(page.locator('.stage')).not.toHaveClass(/menu-open/);
  await expectNoErrors(errors);
});
