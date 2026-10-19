import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

// Carte de validation (KOB-62) : contrat commun des permissions, section 4.

type Appel = { cmd: string; args?: Record<string, unknown> };

/** Coque factice : garde les appels, `decider_carte` peut échouer (carte expirée). */
async function fakeShell(page: Page, expiree = false): Promise<void> {
  await page.addInitScript((expiree) => {
    const calls: Array<{ cmd: string; args?: unknown }> = [];
    (window as unknown as Record<string, unknown>).__calls = calls;
    (window as unknown as Record<string, unknown>).__TAURI__ = {
      window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
      event: { listen: async () => () => {} },
      core: {
        invoke: async (cmd: string, args?: unknown) => {
          calls.push({ cmd, args });
          if (cmd === 'etat_session') return { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false };
          if (cmd === 'etat_ecoute') return false;
          if (cmd === 'decider_carte' && expiree) throw 'carte inconnue : 1';
          if (cmd === 'decider_carte') return null;
          if (cmd === 'ouvrir_app') return null;
          throw `Command ${cmd} not found`;
        },
      },
    };
  }, expiree);
}

const decisions = (page: Page): Promise<Appel[]> =>
  page.evaluate(() => (window as unknown as { __calls: Appel[] }).__calls.filter((c) => c.cmd === 'decider_carte'));

const FICHIER = {
  id: '1',
  type: 'validation',
  genre: 'fichier',
  resume: 'Modifier le fichier notes.md',
  detail: { diff: '--- /code/kobi/notes.md\n+++ /code/kobi/notes.md\n@@\n-a\n-b\n+a\n+c' },
  echeance_ms: 60000,
};
const COMMANDE = { id: '2', type: 'validation', genre: 'commande', resume: 'Pousser la branche', detail: { commande: 'git push', dossier: '/code/kobi' }, echeance_ms: 60000 };
const MCP = { id: '3', type: 'validation', genre: 'mcp', resume: 'linear · save issue : Devis', detail: { serveur: 'linear', outil: 'save_issue', params: { title: 'Devis', team: 'KOB' } }, echeance_ms: 60000 };
const REGLAGE = { id: '4', type: 'validation', genre: 'reglage', resume: 'Changer le réglage « voice.meeting » : false → true', detail: { cle: 'voice.meeting', avant: false, apres: true }, echeance_ms: 60000 };
const SUPPRESSION = { id: '5', type: 'validation', genre: 'suppression', resume: 'Supprimer : rm -rf build', detail: { commande: 'rm -rf build' }, echeance_ms: 60000 };

const carte = (page: Page, c: object): Promise<void> => page.evaluate((c) => window.kobi.carte(c as never), c);
const dialog = (page: Page) => page.getByRole('alertdialog');

test('clic sur Valider : decider_carte accepte, issue puis départ', async ({ page }) => {
  await fakeShell(page);
  const errors = await openUi(page, '');
  await carte(page, FICHIER);
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page)).toContainText('Écrire un fichier');
  await expect(dialog(page)).toContainText('Modifier le fichier notes.md');
  await expect(dialog(page)).toContainText('60 s');
  // Sobriété : la cible sur une ligne, pas de diff à côté de Kobi.
  await expect(page.locator('.kval-cible')).toHaveText('/code/kobi/notes.md');
  await expect(page.locator('.kval .kval-diff')).toHaveCount(0);
  await page.getByRole('button', { name: 'Valider' }).click();
  expect(await decisions(page)).toEqual([{ cmd: 'decider_carte', args: { id: '1', accepte: true } }]);
  await expect(page.locator('.kval')).toContainText('Validé.');
  await expect(page.locator('.kval')).toHaveCount(0, { timeout: 4000 });
  await expectNoErrors(errors);
});

test('Entrée valide, Échap refuse ; les cartes suivantes attendent leur tour', async ({ page }) => {
  await fakeShell(page);
  const errors = await openUi(page, '');
  await carte(page, COMMANDE);
  await carte(page, MCP);
  await expect(page.locator('.kval-badge')).toHaveText('+1');
  await expect(dialog(page)).toContainText('Lancer une commande');
  await expect(page.locator('.kval-cible')).toHaveText('$ git push');
  // « Détail dans Kobi » ouvre la section Activité de l'app ; le focus sur ce
  // lien n'empêche pas Entrée de valider.
  await page.getByRole('button', { name: 'Détail dans Kobi' }).click();
  await page.keyboard.press('Enter');
  await expect(page.locator('.kval')).toContainText('Validé.');
  // La suivante prend la place.
  await expect(dialog(page)).toContainText('Agir dans un service', { timeout: 4000 });
  await expect(page.locator('.kval-badge')).toBeHidden();
  await expect(page.locator('.kval-cible')).toHaveText('linear · save_issue');
  await page.keyboard.press('Escape');
  await expect(page.locator('.kval')).toContainText('Refusé.');
  expect(await decisions(page)).toEqual([
    { cmd: 'decider_carte', args: { id: '2', accepte: true } },
    { cmd: 'decider_carte', args: { id: '3', accepte: false } },
  ]);
  const ouvertures = await page.evaluate(() => (window as unknown as { __calls: Appel[] }).__calls.filter((c) => c.cmd === 'ouvrir_app'));
  expect(ouvertures).toEqual([{ cmd: 'ouvrir_app', args: { section: 'activite' } }]);
  // Échap sur la carte n'a pas fermé autre chose ni coupé de réponse.
  await expect(page.locator('.kval')).toHaveCount(0, { timeout: 4000 });
  await expectNoErrors(errors);
});

test('Entrée et Échap dans la bulle d’écriture ne tranchent pas la carte', async ({ page }) => {
  await fakeShell(page);
  const errors = await openUi(page, '');
  await carte(page, REGLAGE);
  // Bulle d'écriture de Kobi (KOB-147) ouverte, focus dans le champ.
  await page.evaluate(() => window.kobi.ecrire());
  const champ = page.getByRole('textbox', { name: 'Écrire à Kobi' });
  await expect(champ).toBeFocused();
  await champ.fill('une question');
  await champ.press('Enter');
  await champ.press('Escape');
  expect(await decisions(page)).toEqual([]);
  await expect(dialog(page)).toBeVisible();
  await expect(page.locator('.kval-cible')).toHaveText('voice.meeting : false → true');
  await expectNoErrors(errors);
});

test('ignorée : refus annoncé à 60 s, sans appel de la coque', async ({ page }) => {
  await page.clock.install();
  await fakeShell(page);
  const errors = await openUi(page, '');
  await carte(page, SUPPRESSION);
  await expect(dialog(page)).toContainText('Supprimer');
  await expect(page.getByRole('button', { name: 'Supprimer' })).toBeVisible();
  await page.clock.runFor(49_500);
  await expect(page.locator('.kval-secs')).toHaveText('11 s');
  await expect(page.locator('.kval')).not.toHaveClass(/urgent/);
  await page.clock.runFor(1_000);
  await expect(page.locator('.kval-secs')).toHaveText('10 s');
  await expect(page.locator('.kval')).toHaveClass(/urgent/);
  await page.clock.runFor(9_600);
  await expect(page.locator('.kval')).toContainText('Pas de réponse : refusé.');
  expect(await decisions(page)).toEqual([]);
  await page.clock.runFor(3_000);
  await expect(page.locator('.kval')).toHaveCount(0);
  await expectNoErrors(errors);
});

test('tranchée par la voix : la coque republie la carte avec son statut', async ({ page }) => {
  await fakeShell(page);
  const errors = await openUi(page, '');
  await carte(page, FICHIER);
  await expect(dialog(page)).toBeVisible();
  await carte(page, { ...FICHIER, statut: 'acceptee' });
  await expect(page.locator('.kval')).toContainText('Validé.');
  expect(await decisions(page)).toEqual([]);
  // Une republication tardive ne rouvre pas la carte.
  await expect(page.locator('.kval')).toHaveCount(0, { timeout: 4000 });
  await carte(page, FICHIER);
  await expect(page.locator('.kval')).toHaveCount(0);
  await expectNoErrors(errors);
});

test('clic trop tard : la coque ne connaît plus la carte, refus affiché', async ({ page }) => {
  await fakeShell(page, true);
  const errors = await openUi(page, '');
  await carte(page, COMMANDE);
  await page.getByRole('button', { name: 'Valider' }).click();
  await expect(page.locator('.kval')).toContainText('Pas de réponse : refusé.');
  await expectNoErrors(errors);
});

test('boutons immobiles : même place avant et pendant le survol, animations réduites respectées', async ({ page }) => {
  await fakeShell(page);
  const errors = await openUi(page, 'reduced=1');
  await carte(page, COMMANDE);
  const valider = page.getByRole('button', { name: 'Valider' });
  await expect(dialog(page)).toBeVisible();
  // Animations réduites : aucune animation sur la carte.
  const anims = await page.evaluate(() => document.querySelector('.kval')!.getAnimations({ subtree: true }).length);
  expect(anims).toBe(0);
  const avant = await valider.boundingBox();
  await valider.hover();
  await page.waitForTimeout(300);
  expect(await valider.boundingBox()).toEqual(avant);
  await expectNoErrors(errors);
});

test('boutons immobiles avec animations : la carte posée ne bouge plus', async ({ page }) => {
  await fakeShell(page);
  const errors = await openUi(page, '');
  await carte(page, FICHIER);
  const valider = page.getByRole('button', { name: 'Valider' });
  await page.waitForTimeout(700); // fin de l'entrée
  const avant = await valider.boundingBox();
  await valider.hover();
  await page.waitForTimeout(1500);
  expect(await valider.boundingBox()).toEqual(avant);
  await expectNoErrors(errors);
});

test('cartes en attente de leur tour : l’échéance court depuis leur arrivée ; Espace sur un bouton le clique', async ({ page }) => {
  await page.clock.install();
  await fakeShell(page);
  const errors = await openUi(page, '');
  await carte(page, COMMANDE);
  await carte(page, { ...MCP, echeance_ms: 20_000 });
  await carte(page, { ...REGLAGE, echeance_ms: 30_000 });
  await expect(page.locator('.kval-badge')).toHaveText('+2');
  await page.clock.runFor(25_000);
  // Espace sur « Refuser » : la carte est refusée, la bulle d'écriture reste fermée.
  await page.getByRole('button', { name: 'Refuser' }).focus();
  await page.keyboard.press(' ');
  await expect(page.locator('.kval')).toContainText('Refusé.');
  await expect(page.getByRole('textbox', { name: 'Écrire à Kobi' })).toBeHidden();
  await page.clock.runFor(2_000);
  // La carte MCP (20 s) a expiré pendant l'attente : on passe au réglage, à qui il reste ~3 s.
  await expect(page.locator('.kval')).toContainText('Changer un réglage');
  await expect(page.locator('.kval-secs')).toHaveText(/^[1-4] s$/);
  await expect(page.locator('.kval')).toHaveClass(/urgent/);
  expect(await decisions(page)).toEqual([{ cmd: 'decider_carte', args: { id: '2', accepte: false } }]);
  await expectNoErrors(errors);
});
