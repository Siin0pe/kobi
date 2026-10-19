// « À valider » dans la section Activité de l'app Kobi (KOB-62) : le détail
// complet des cartes de validation en attente, et les mêmes réponses.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors } from './helpers';

const CARTES = [
  {
    id: '7',
    type: 'validation',
    genre: 'fichier',
    resume: 'Modifier le fichier chef.rs',
    detail: { diff: '--- crates/kobi-app/src/chef.rs\n+++ crates/kobi-app/src/chef.rs\n@@\n-    intervalle: 30,\n+    intervalle: 20,' },
    echeance_ms: 60000,
  },
  { id: '8', type: 'validation', genre: 'mcp', resume: 'linear · save issue : Devis', detail: { serveur: 'linear', outil: 'save_issue', params: { title: 'Devis', team: 'KOB' } }, echeance_ms: 60000 },
];

function fakeTauri(cartes: unknown[]): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        switch (cmd) {
          case 'cartes_validation':
            return cartes;
          case 'decider_carte':
            return null;
          case 'historique':
            return [];
          case 'carnet_index':
            return { racine: '/home/test/Kobi', reunions: [], conversations: [], rappels: [] };
          default:
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

async function ouvrirActivite(page: Page, cartes: unknown[]): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri, cartes);
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto('/app.html#activite');
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

const bloc = (page: Page) => page.getByRole('region', { name: 'Actions à valider' });
const decisions = (page: Page) => page.evaluate(() => (window as unknown as { __appels: Array<[string, unknown]> }).__appels.filter(([c]) => c === 'decider_carte'));

test('les cartes en attente avec leur détail complet ; Valider et Refuser répondent à la coque', async ({ page }) => {
  const errors = await ouvrirActivite(page, CARTES);
  await expect(bloc(page)).toBeVisible();
  const fichier = page.locator('.validation[data-id="7"]');
  await expect(fichier).toContainText('Écrire un fichier');
  await expect(fichier).toContainText('Modifier le fichier chef.rs');
  await expect(fichier.locator('.validation-cible')).toHaveText('crates/kobi-app/src/chef.rs');
  await expect(fichier.locator('.kval-diff .del')).toHaveText(['-    intervalle: 30,']);
  await expect(fichier.locator('.kval-diff .add')).toHaveText(['+    intervalle: 20,']);
  const mcp = page.locator('.validation[data-id="8"]');
  await expect(mcp.locator('.kval-params')).toContainText('"team": "KOB"');
  // Boutons immobiles au survol (une fois la section posée).
  const valider = fichier.getByRole('button', { name: 'Valider' });
  await page.waitForTimeout(1200);
  const avant = await valider.boundingBox();
  await valider.hover();
  await page.waitForTimeout(400);
  expect(await valider.boundingBox()).toEqual(avant);
  await valider.click();
  await expect(fichier).toHaveCount(0);
  await mcp.getByRole('button', { name: 'Refuser' }).click();
  await expect(mcp).toHaveCount(0);
  expect(await decisions(page)).toEqual([
    ['decider_carte', { id: '7', accepte: true }],
    ['decider_carte', { id: '8', accepte: false }],
  ]);
  // Plus rien à valider : le bloc disparaît, le journal reste.
  await expect(bloc(page)).toBeHidden();
  await expectNoErrors(errors);
});

test('suit l’événement carte : nouvelle carte ajoutée, carte tranchée par la voix retirée', async ({ page }) => {
  const errors = await ouvrirActivite(page, []);
  await expect(page.locator('.vue-activite')).toBeVisible();
  await expect(bloc(page)).toBeHidden();
  await page.evaluate((c) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('carte', c), CARTES[1]);
  await expect(page.locator('.validation[data-id="8"]')).toBeVisible();
  // Les autres cartes (confirmation d'action, KOB-74) ne s'affichent pas ici.
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('carte', { id: '9', kicker: 'Linear', title: 'Devis', meta: null, status: 'pending' }));
  await expect(page.locator('.validation')).toHaveCount(1);
  await page.evaluate((c) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('carte', { ...c, statut: 'acceptee' }), CARTES[1]);
  await expect(page.locator('.validation')).toHaveCount(0);
  await expect(bloc(page)).toBeHidden();
  expect(await decisions(page)).toEqual([]);
  await expectNoErrors(errors);
});
