// Réglages → Outils MCP (KOB-155) avec une fausse coque Tauri. Le classement,
// l'import et les secrets sont testés côté Rust (crates/kobi-app/src/mcp.rs,
// crates/kobi-perms/src/mcp.rs).
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors } from './helpers';

function fakeTauri(): void {
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  const serveur = (nom: string, extra: Record<string, unknown> = {}) => ({
    nom,
    transport: 'http',
    resume: `https://mcp.${nom}.app/mcp`,
    actif: true,
    ecriture: 'demander',
    origine: 'Claude Code (utilisateur)',
    commande: '',
    args: [],
    url: `https://mcp.${nom}.app/mcp`,
    env: {},
    entetes: {},
    secrets: [],
    outils: [],
    teste_ms: null,
    ...extra,
  });
  let serveurs: Array<Record<string, unknown>> = [serveur('linear')];
  const trouver = (nom: unknown) => serveurs.find((s) => s.nom === nom)!;
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        switch (cmd) {
          case 'mcp_serveurs':
            return serveurs;
          case 'mcp_candidats':
            return [
              { nom: 'linear', portee: 'utilisateur', transport: 'http', resume: 'https://mcp.linear.app/mcp', deja: true, secrets: [] },
              { nom: 'treg', portee: 'utilisateur', transport: 'http', resume: 'https://treg.to/mcp/', deja: false, secrets: ['Authorization'] },
              { nom: 'notes', portee: 'projet · C:/code/site', transport: 'stdio', resume: 'node', deja: false, secrets: [] },
            ];
          case 'mcp_importer':
            for (const nom of args.noms as string[]) serveurs.push(serveur(nom, { secrets: nom === 'treg' ? [{ nom: 'Authorization', present: true }] : [] }));
            return serveurs;
          case 'mcp_regler': {
            Object.assign(trouver(args.nom), Object.fromEntries(Object.entries(args.changement as object).filter(([, v]) => v !== undefined)));
            return serveurs;
          }
          case 'mcp_tester': {
            if (args.nom === 'treg') return { etat: 'autorisation', message: 'Ce serveur demande une connexion.' };
            const outils = [
              { nom: 'list_issues', description: 'Liste les tickets.', nature: 'lecture' },
              { nom: 'save_issue', description: 'Crée ou modifie un ticket.', nature: 'ecriture' },
              { nom: 'delete_comment', description: '', nature: 'suppression' },
            ];
            trouver(args.nom).outils = outils.map(({ nom, nature }) => ({ nom, nature }));
            return { etat: 'ok', outils };
          }
          case 'mcp_autoriser':
            return { etat: 'ok', outils: [{ nom: 'search', description: '', nature: 'lecture' }] };
          case 'mcp_ajouter': {
            const s = args.serveur as Record<string, unknown>;
            if (serveurs.some((x) => x.nom === s.nom)) throw `« ${s.nom} » existe déjà`;
            serveurs.push(serveur(String(s.nom), { transport: s.transport, resume: s.transport === 'stdio' ? s.commande : s.url, origine: 'ajouté dans Kobi', secrets: Object.keys(s.secrets as object).map((nom) => ({ nom, present: true })) }));
            return serveurs;
          }
          case 'mcp_supprimer':
            serveurs = serveurs.filter((s) => s.nom !== args.nom);
            return serveurs;
          default:
            throw new Error(`commande inconnue ${cmd}`);
        }
      },
    },
    event: { listen: async () => () => {} },
  };
}

async function ouvrir(page: Page): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.addInitScript(fakeTauri);
  await page.setViewportSize({ width: 960, height: 760 });
  await page.goto('/reglages.html?panneau=mcp');
  await expect(page.locator('.titre')).toHaveText('Outils MCP');
  return errors;
}

const appels = (page: Page) => page.evaluate(() => (window as unknown as { __appels: Array<[string, unknown]> }).__appels);
const carte = (page: Page, nom: string) => page.locator(`.mcp-serveur[data-nom="${nom}"]`);

test('tester un serveur : outils listés avec ce qu’ils font', async ({ page }) => {
  const errors = await ouvrir(page);
  await expect(carte(page, 'linear')).toContainText('pas encore testé');
  await carte(page, 'linear').getByRole('button', { name: 'Tester' }).click();
  await expect(carte(page, 'linear').locator('.etat[data-statut="ok"]')).toContainText('3 outils · 1 lit · 1 écrit · 1 supprime');
  await expect(carte(page, 'linear').locator('.mcp-outil')).toHaveText(['list_issueslit', 'save_issueécrit', 'delete_commentsupprime']);
  await expect(carte(page, 'linear').locator('.mcp-outil[data-nature="lecture"]')).toHaveCount(1);
  await expectNoErrors(errors);
});

test('écriture par serveur et activation', async ({ page }) => {
  const errors = await ouvrir(page);
  const linear = carte(page, 'linear');
  await expect(linear.getByRole('radio', { name: 'Toujours demander' })).toHaveAttribute('aria-checked', 'true');
  await linear.getByRole('radio', { name: 'Faire sans demander' }).click();
  expect(await appels(page)).toContainEqual(['mcp_regler', { nom: 'linear', changement: { ecriture: 'sans_demander' } }]);
  await expect(carte(page, 'linear').getByRole('radio', { name: 'Faire sans demander' })).toHaveAttribute('aria-checked', 'true');
  await carte(page, 'linear').getByRole('switch').click();
  expect(await appels(page)).toContainEqual(['mcp_regler', { nom: 'linear', changement: { actif: false } }]);
  await expect(carte(page, 'linear')).toHaveAttribute('data-actif', 'false');
  await expectNoErrors(errors);
});

test('import depuis Claude Code : déjà connecté grisé, secrets signalés', async ({ page }) => {
  const errors = await ouvrir(page);
  const deja = page.locator('.mcp-candidat[data-deja="true"]');
  await expect(deja).toContainText('linear');
  await expect(deja.locator('input')).toBeDisabled();
  await expect(page.locator('.mcp-candidat', { hasText: 'treg' }).locator('.puce')).toHaveText('secret');
  // On ne garde que treg.
  await page.locator('.mcp-candidat input[data-nom="notes"]').uncheck();
  await page.getByRole('button', { name: 'Importer 1 serveur' }).click();
  expect(await appels(page)).toContainEqual(['mcp_importer', { noms: ['treg'] }]);
  await expect(carte(page, 'treg')).toBeVisible();
  await expect(carte(page, 'treg').locator('.mcp-secrets')).toContainText('Authorization');
  await expect(carte(page, 'notes')).toHaveCount(0);
  await expectNoErrors(errors);
});

test('serveur protégé : Kobi demande sa propre autorisation', async ({ page }) => {
  const errors = await ouvrir(page);
  await page.getByRole('button', { name: /Importer/ }).click();
  await carte(page, 'treg').getByRole('button', { name: 'Tester' }).click();
  await expect(carte(page, 'treg').locator('.etat[data-statut="attention"]')).toContainText('demande une connexion');
  await carte(page, 'treg').getByRole('button', { name: 'Autoriser Kobi' }).click();
  await expect(carte(page, 'treg').locator('.etat[data-statut="ok"]')).toContainText('1 outil · 1 lit');
  await expectNoErrors(errors);
});

test('ajout à la main, doublon refusé, retrait', async ({ page }) => {
  const errors = await ouvrir(page);
  await page.locator('.mcp-ajout summary').click();
  await page.locator('input[name="nom"]').fill('notion');
  await page.locator('input[name="url"]').fill('https://mcp.notion.com/mcp');
  await page.locator('textarea[name="entetes"]').fill('Authorization: Bearer abc');
  await page.getByRole('button', { name: 'Ajouter', exact: true }).click();
  const ajout = (await appels(page)).find(([c]) => c === 'mcp_ajouter')!;
  expect(ajout[1]).toMatchObject({ serveur: { nom: 'notion', transport: 'http', url: 'https://mcp.notion.com/mcp', secrets: { Authorization: 'Bearer abc' } } });
  await expect(carte(page, 'notion')).toBeVisible();
  // Local : commande, arguments, variables.
  await page.getByRole('radio', { name: 'Commande locale' }).click();
  await page.locator('input[name="nom"]').fill('linear');
  await page.locator('input[name="commande"]').fill('npx');
  await page.getByRole('button', { name: 'Ajouter', exact: true }).click();
  await expect(page.locator('.notif').last()).toContainText('existe déjà');
  await carte(page, 'notion').getByRole('button', { name: 'Retirer' }).click();
  await expect(carte(page, 'notion')).toHaveCount(0);
  await expectNoErrors(errors);
});
