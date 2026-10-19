// Son de l'ordinateur (KOB-164) : panneau de la section Réglages de l'app et
// indicateur discret sur l'étiquette REC. La capture elle-même est testée
// côté Rust (crates/kobi-app/src/son_ordi.rs) et sous Windows.
import { expect, test } from '@playwright/test';
import { expectNoErrors, mockTauri, openUi } from './helpers';

function fakeTauri(): void {
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  let etat = { actif: false, sortie: null as unknown, capte: false, sorties: [
      { nom: 'Haut-parleurs (Realtek)', libelle: 'Haut-parleurs (Realtek)' },
      // Comme sous Linux (KOB-193) : le sink a un nom technique, l'utilisateur voit sa description.
      { nom: 'alsa_output.usb-casque.analog-stereo', libelle: 'Casque USB' },
    ],
    defaut: 'Haut-parleurs (Realtek)',
  };
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        if (cmd === 'son_ordi') return etat;
        if (cmd === 'regler_son_ordi') {
          etat = { ...etat, actif: Boolean(args.actif), sortie: args.actif ? (args.sortie ?? null) : null };
          return etat;
        }
        return null;
      },
    },
    event: { listen: async () => () => {} },
  };
}

const appels = (page: import('@playwright/test').Page) => page.evaluate(() => (window as unknown as { __appels: Array<[string, unknown]> }).__appels);

test('Réglages → Son de l’ordinateur : désactivé par défaut, activé, sortie choisie, désactivé', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.goto('/app.html#reglages/son-ordi');
  await expect(page.locator('.feuille .titre')).toHaveText('Son de l’ordinateur');
  const interrupteur = page.getByRole('switch', { name: 'Écouter aussi le son de l’ordinateur' });
  await expect(interrupteur).toHaveAttribute('aria-checked', 'false');
  await expect(page.getByRole('radiogroup', { name: 'Sortie audio écoutée' })).toHaveCount(0);
  await interrupteur.click();
  expect(await appels(page)).toContainEqual(['regler_son_ordi', { actif: true, sortie: null }]);
  await expect(page.getByRole('switch', { name: 'Écouter aussi le son de l’ordinateur' })).toHaveAttribute('aria-checked', 'true');
  await expect(page.getByRole('radio', { name: /Celle du système/ })).toHaveAttribute('aria-checked', 'true');
  await expect(page.getByRole('radio')).toHaveCount(3);
  await page.getByRole('radio', { name: 'Casque USB' }).click();
  expect(await appels(page)).toContainEqual(['regler_son_ordi', { actif: true, sortie: 'alsa_output.usb-casque.analog-stereo' }]);
  await expect(page.getByRole('radio', { name: 'Casque USB' })).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('switch', { name: 'Écouter aussi le son de l’ordinateur' }).click();
  expect(await appels(page)).toContainEqual(['regler_son_ordi', { actif: false, sortie: 'alsa_output.usb-casque.analog-stereo' }]);
  await expect(page.getByRole('switch', { name: 'Écouter aussi le son de l’ordinateur' })).toHaveAttribute('aria-checked', 'false');
  await expectNoErrors(errors);
});

test('en réunion, « son de l’ordi » s’affiche sur REC seulement quand il est capté', async ({ page }) => {
  await mockTauri(page, {
    etat_enregistrement: { actif: true, pause: false, debut_ms: Date.now(), notes: 0, titre: null },
    son_ordi: { actif: true, sortie: null, capte: true, sorties: [], defaut: null },
  });
  const errors = await openUi(page, '');
  const eux = page.locator('.rec-eux');
  await expect(eux).toBeVisible();
  await expect(eux).toHaveText('son de l’ordi');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('son_ordi', false));
  await expect(eux).toBeHidden();
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('son_ordi', true));
  await expect(eux).toBeVisible();
  await expectNoErrors(errors);
});
