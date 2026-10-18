// L'app Kobi (KOB-159) : une seule fenêtre, ses sections et leur adresse.
// Fausse coque Tauri ; l'ouverture côté Rust est testée dans app.rs.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors } from './helpers';

function fakeTauri(): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  const rappels = [
    { id: 'r1', texte: 'Appeler Léa', echeance_ms: Date.now() + 3_600_000, cree_ms: Date.now(), fait: false, source: null },
    { id: 'r2', texte: 'Envoyer le devis', echeance_ms: null, cree_ms: Date.now(), fait: true, source: null },
  ];
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        switch (cmd) {
          case 'carnet_index':
            return { racine: '/home/test/Kobi', reunions: [], conversations: [], rappels };
          case 'carnet_rappel_fait':
            rappels.find((r) => r.id === args.id)!.fait = true;
            return true;
          case 'reglages_stockage':
            return {
              dossier: '/home/test/Kobi',
              par_defaut: true,
              retention_reunions: 'unlimited',
              retention_conversations: 'unlimited',
              publication_reunions: { etat: 'aucune' },
              publication_conversations: { etat: 'aucune' },
            };
          case 'a_propos':
            return { version: '0.1.0', dossier: '/home/test/Kobi' };
          default:
            // Les étapes de l'accueil ont leurs propres tests (bienvenue.spec.ts).
            throw `Command ${cmd} not found`;
        }
      },
    },
    event: {
      listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
        (listeners[event] ??= []).push(handler);
        return () => {};
      },
    },
  };
}

async function ouvrirApp(page: Page, ancre = ''): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto(`/app.html${ancre}`);
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

const visible = (page: Page) => page.locator('.vue:not([hidden])');

test('navigation entre toutes les sections, adresse dans l’URL', async ({ page }) => {
  const errors = await ouvrirApp(page);
  // Par défaut : Notes et réunions (le Carnet).
  await expect(visible(page)).toHaveAttribute('data-section', 'notes');
  await expect(page.locator('.vue-notes .carnet')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Notes et réunions' })).toHaveAttribute('aria-current', 'page');
  for (const [nom, id] of [
    ['Rappels et tâches', 'rappels'],
    ['Activité', 'activite'],
    ['Réglages', 'reglages'],
    ['À propos', 'a-propos'],
    ['Notes et réunions', 'notes'],
  ] as const) {
    await page.getByRole('button', { name: nom, exact: true }).click();
    await expect(visible(page)).toHaveAttribute('data-section', id);
    await expect(page).toHaveURL(new RegExp(`#${id}`));
    await expect(page.getByRole('button', { name: nom, exact: true })).toHaveAttribute('aria-current', 'page');
  }
  await expect(page).toHaveTitle('Kobi · Notes et réunions');
  await expectNoErrors(errors);
});

test('app::ouvrir : adresse de départ et événement app://section', async ({ page }) => {
  const errors = await ouvrirApp(page, '#reglages/stockage');
  await expect(visible(page)).toHaveAttribute('data-section', 'reglages');
  await expect(page.locator('.vue-reglages .titre')).toHaveText('Stockage');
  // L'app est déjà ouverte : Rust envoie la section voulue.
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('app://section', 'rappels'));
  await expect(visible(page)).toHaveAttribute('data-section', 'rappels');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('app://section', 'a-propos'));
  await expect(page.locator('.vue-a-propos .infos')).toContainText('0.1.0');
  await expectNoErrors(errors);
});

test('premier démarrage : l’accueil occupe toute la fenêtre, puis l’app', async ({ page }) => {
  await ouvrirApp(page, '#accueil');
  await expect(page.locator('.vue-accueil')).toBeVisible();
  await expect(page.locator('.vue-accueil .titre')).toHaveText('Salut, moi c’est Kobi.');
  await expect(page.locator('.nav')).toBeHidden();
  // Quitter l'accueil par la navigation (adresse) : la nav revient.
  await page.evaluate(() => window.kobiApp.aller('notes'));
  await expect(page.locator('.nav')).toBeVisible();
  await expect(page.locator('.vue-accueil')).toBeHidden();
  await expect(visible(page)).toHaveAttribute('data-section', 'notes');
});

test('rappels et tâches : à venir, faits, et cocher', async ({ page }) => {
  const errors = await ouvrirApp(page, '#rappels');
  await expect(page.locator('.vue-rappels .groupe')).toHaveText(['À venir (1)', 'Faits (1)']);
  await page.getByRole('button', { name: 'Marquer « Appeler Léa » comme fait' }).click();
  await expect(page.locator('.notif')).toHaveText('Rappel fait.');
  await expect(page.locator('.vue-rappels .groupe')).toHaveText(['À venir (0)', 'Faits (2)']);
  await expectNoErrors(errors);
});

test('un écran s’ajoute aux Réglages sans toucher à l’app (ajouterPanneau)', async ({ page }) => {
  const errors = await ouvrirApp(page);
  await page.evaluate(() =>
    window.kobiApp.ajouterPanneau(
      {
        id: 'mcp',
        titre: 'Outils (MCP)',
        sousTitre: 'Connecter tes outils',
        monter: (el: HTMLElement) => void el.replaceChildren(Object.assign(document.createElement('p'), { textContent: 'Panneau ajouté.' })),
      },
      'stockage',
    ),
  );
  await page.evaluate(() => window.kobiApp.aller('reglages/mcp'));
  await expect(page.locator('.vue-reglages .titre')).toHaveText('Outils (MCP)');
  await expect(page.locator('.vue-reglages .panneau')).toHaveText('Panneau ajouté.');
  // Rangé juste après Stockage dans le menu.
  await expect(page.locator('.vue-reglages .menu-entree').nth(1)).toHaveAttribute('data-panneau', 'mcp');
  await expectNoErrors(errors);
});
