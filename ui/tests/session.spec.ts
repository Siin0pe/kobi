import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

// Fausse coque Tauri : `window.__TAURI__` minimal qui rejoue la session de
// `kobi-app` (commandes → événements `state`). Le comportement réel des
// commandes est testé côté Rust (crates/kobi-app/src/session.rs).
function fakeTauri(): void {
  type State = { etat: string; activite: string | null; mascotte: string; transcrit: boolean };
  const S = {
    veille: { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false },
    reunion: { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true },
    pause: { etat: 'pause', activite: null, mascotte: 'idle', transcrit: false },
    fin: { etat: 'fin', activite: null, mascotte: 'write', transcrit: false },
  } satisfies Record<string, State>;
  const listeners: Record<string, ((e: { payload: unknown }) => void)[]> = {};
  const emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  let current: State = S.veille;
  const go = (...steps: State[]) => {
    for (const s of steps) {
      current = s;
      emit('state', s);
    }
    return current;
  };
  const allowed: Record<string, [string, () => State]> = {
    demarrer: ['veille', () => go(S.reunion)],
    pause: ['reunion', () => go(S.pause)],
    reprendre: ['pause', () => go(S.reunion)],
    terminer: ['reunion', () => go(S.fin, S.veille)],
  };
  const w = window as unknown as Record<string, unknown>;
  w.__invokes = [] as string[];
  w.__emit = emit;
  w.__TAURI__ = {
    window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
    core: {
      invoke: async (cmd: string) => {
        (w.__invokes as string[]).push(cmd);
        if (cmd === 'etat_session') return current;
        const rule = allowed[cmd];
        if (!rule) throw new Error(`commande inconnue ${cmd}`);
        if (rule[0] !== current.etat) throw `transition refusée : ${cmd} dans l'état ${current.etat}`;
        return rule[1]();
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

test('démarrer puis terminer une réunion depuis la webview', async ({ page }) => {
  await page.addInitScript(fakeTauri);
  const errors = await openUi(page, 'skin=kobi');
  const root = page.locator('[data-session]');
  const skin = page.locator('.skin-kobi');

  await expect(root).toHaveAttribute('data-session', 'veille');
  await expect(root).toHaveAttribute('data-rec', 'false');

  const started = await page.evaluate(() => window.kobi.command('demarrer'));
  expect(started).toMatchObject({ etat: 'reunion', transcrit: true });
  await expect(root).toHaveAttribute('data-session', 'reunion');
  await expect(root).toHaveAttribute('data-rec', 'true');

  // La mascotte suit l'état envoyé par la session.
  await page.evaluate(() =>
    (window as unknown as { __emit(e: string, p: unknown): void }).__emit('state', {
      etat: 'reunion',
      activite: 'reflechit',
      mascotte: 'think',
      transcrit: true,
    }),
  );
  await expect(skin).toHaveAttribute('data-state', 'think');

  // Les segments sont relayés en événement DOM.
  const segment = await page.evaluate(
    () =>
      new Promise((resolve) => {
        window.addEventListener('kobi:segment', (e) => resolve((e as CustomEvent).detail), { once: true });
        (window as unknown as { __emit(e: string, p: unknown): void }).__emit('segment', {
          start: 1,
          end: 2,
          source: 'eux',
          text: 'bonjour',
          is_final: true,
        });
      }),
  );
  expect(segment).toMatchObject({ source: 'eux', text: 'bonjour' });

  await page.evaluate(() => window.kobi.command('pause'));
  await expect(root).toHaveAttribute('data-session', 'pause');
  await expect(root).toHaveAttribute('data-rec', 'false');
  await page.evaluate(() => window.kobi.command('reprendre'));

  const ended = await page.evaluate(() => window.kobi.command('terminer'));
  expect(ended).toMatchObject({ etat: 'veille', transcrit: false });
  await expect(root).toHaveAttribute('data-session', 'veille');
  await expect(skin).toHaveAttribute('data-state', 'idle');

  // Un refus de la session revient en erreur, sans changer l'affichage.
  const refusal = await page.evaluate(() => window.kobi.command('pause').then(() => 'ok', (e) => String(e)));
  expect(refusal).toContain('transition refusée');
  await expect(root).toHaveAttribute('data-session', 'veille');

  const invokes = await page.evaluate(() => (window as unknown as { __invokes: string[] }).__invokes);
  expect(invokes).toEqual(['etat_session', 'demarrer', 'pause', 'reprendre', 'terminer', 'pause']);
  await expectNoErrors(errors);
});

test('hors de Tauri, les commandes ne font rien', async ({ page }) => {
  const errors = await openUi(page, 'skin=kobi');
  expect(await page.evaluate(() => window.kobi.command('demarrer'))).toBeUndefined();
  await expect(page.locator('[data-session]')).toHaveCount(0);
  await expectNoErrors(errors);
});
