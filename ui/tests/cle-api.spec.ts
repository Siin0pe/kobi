// Clé API (KOB-104) : panneau des Réglages. La coque garde la clé dans le
// gestionnaire d'identifiants et n'en renvoie que la fin ; le stockage lui-même
// est testé côté Rust (crates/kobi-app/src/cle_api.rs).
import { expect, test } from '@playwright/test';
import { expectNoErrors } from './helpers';

function fakeTauri(): void {
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  let cle: string | null = null;
  const etat = () => ({ presente: cle !== null, fin: cle ? cle.slice(-4) : null });
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        if (cmd === 'cle_api_etat') return etat();
        if (cmd === 'cle_api_enregistrer') {
          const c = String(args.cle).trim();
          if (!c.startsWith('sk-ant-')) throw 'Cette clé ne ressemble pas à une clé API Anthropic.';
          cle = c;
          return etat();
        }
        if (cmd === 'cle_api_supprimer') {
          cle = null;
          return etat();
        }
        return null;
      },
    },
    event: { listen: async () => () => {} },
  };
}

test('Réglages → Clé API : saisie, la clé n’est jamais réaffichée, remplacement, suppression, erreur dite', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.goto('/app.html#reglages/cle-api');
  await expect(page.locator('.feuille .titre')).toHaveText('Clé API');
  const etat = page.locator('.cle-api-etat');
  await expect(etat).toHaveText('Aucune clé enregistrée.');
  const champ = page.getByLabel('Clé API Anthropic');
  await expect(champ).toHaveAttribute('type', 'password');
  await champ.fill('sk-ant-api03-ABCDEFGHIJKLMNOP-1234');
  await page.getByRole('button', { name: 'Enregistrer' }).click();
  await expect(etat).toHaveText('Clé enregistrée (…1234).');
  // Le champ est vidé : la clé ne reste nulle part dans la page.
  await expect(page.getByLabel('Clé API Anthropic')).toHaveValue('');
  expect(await page.content()).not.toContain('ABCDEFGHIJKLMNOP');
  // Remplacement, par Entrée.
  await page.getByLabel('Clé API Anthropic').fill('sk-ant-api03-ZZZZZZZZZZZZZZZZ-9876');
  await page.getByLabel('Clé API Anthropic').press('Enter');
  await expect(etat).toHaveText('Clé enregistrée (…9876).');
  // Une valeur qui n'est pas une clé : refusée, l'état ne change pas.
  await page.getByLabel('Clé API Anthropic').fill('bonjour');
  await page.getByRole('button', { name: 'Remplacer' }).click();
  await expect(page.locator('.notif').filter({ hasText: 'ne ressemble pas' })).toBeVisible();
  await expect(etat).toHaveText('Clé enregistrée (…9876).');
  await page.getByRole('button', { name: 'Supprimer la clé' }).click();
  await expect(etat).toHaveText('Aucune clé enregistrée.');
  await expect(page.getByRole('button', { name: 'Supprimer la clé' })).toHaveCount(0);
  await expectNoErrors(errors);
});
