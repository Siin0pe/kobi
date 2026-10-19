// KOB-187 : la liste « Notes et réunions » de l'app suit la fin de la
// réunion (badge REC, durée, compte rendu) sans attendre un retour sur la
// fenêtre. Fausse coque : l'index du Carnet change comme sur le disque.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors } from './helpers';

const DEBUT = Date.now() - 4 * 60_000;

function fakeTauri(debut: number): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const w = window as unknown as Record<string, unknown>;
  w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  const reunion = { nom: '2026-10-19-reunion', date: '2026-10-19', titre: 'Point hebdo', debut_ms: debut, fin_ms: null as number | null, en_cours: true, duree_s: 39, a_compte_rendu: false, apercu: 'Décisions' };
  // La coque termine la réunion : ce que l'index renverra désormais.
  w.__finir = (compteRendu: boolean) => {
    Object.assign(reunion, { en_cours: false, fin_ms: debut + 240_000, duree_s: 240, a_compte_rendu: compteRendu });
  };
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string) => {
        switch (cmd) {
          case 'carnet_index':
            return { racine: '/home/test/Kobi', reunions: [{ ...reunion }], conversations: [], rappels: [] };
          case 'carnet_chercher':
            return [];
          case 'etat_session':
            return { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true };
          case 'etat_enregistrement':
            return { actif: true, pause: false, debut_ms: debut, notes: 3, titre: null };
          case 'notes_en_cours':
            return null;
          case 'journal_activite':
            return [];
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

async function ouvrirNotes(page: Page): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri, DEBUT);
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto('/app.html#notes');
  return errors;
}

const emettre = (page: Page, event: string, payload: unknown) =>
  page.evaluate(([e, p]) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit(e as string, p), [event, payload]);

test('fin de réunion : la liste retire « REC » et donne la durée sans clic (KOB-187)', async ({ page }) => {
  const errors = await ouvrirNotes(page);
  const element = page.locator('.element', { hasText: 'Point hebdo' });
  await expect(element.locator('.rec')).toHaveText('REC');

  // Fin de l'enregistrement, sans focus ni clic : la liste suit.
  await page.evaluate(() => (window as unknown as { __finir: (cr: boolean) => void }).__finir(false));
  await emettre(page, 'enregistrement', { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null });
  await expect(element.locator('.rec')).toHaveCount(0);
  await expect(element).toContainText('4 min');

  // Compte rendu écrit : la liste le dit aussi.
  await page.evaluate(() => (window as unknown as { __finir: (cr: boolean) => void }).__finir(true));
  await emettre(page, 'compte_rendu', { reunion: '2026-10-19-reunion', resume: 'Budget validé.', decisions: 1, actions: 0, questions: 0 });
  await expect(element).toContainText('compte rendu');
  await expectNoErrors(errors);
});
