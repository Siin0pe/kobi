import { expect, type Page } from '@playwright/test';

/** Ouvre l'interface et échoue au moindre message d'erreur dans la console. */
export async function openUi(page: Page, query: string): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.goto(`/?${query}`);
  await page.waitForFunction(() => 'kobi' in window);
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

export async function expectNoErrors(errors: string[]): Promise<void> {
  expect(errors, errors.join('\n')).toEqual([]);
}

/**
 * Repos économe : la police ondule encore (le mot n'est jamais figé), mais
 * lentement, et ses boucles sont tenues par le métronome de la scène
 * (econome.ts) au lieu de produire leurs propres images.
 */
export async function expectEconomicalFont(page: Page): Promise<void> {
  const fonts = await page.evaluate(() =>
    document
      .getAnimations()
      .filter((a) => (a.effect as KeyframeEffect).getKeyframes().some((k) => 'fontVariationSettings' in k))
      .filter((a) => a.playState !== 'finished' && a.playState !== 'idle')
      .map((a) => ({ state: a.playState, duration: Number((a.effect as KeyframeEffect).getTiming().duration) })),
  );
  expect(fonts.length).toBeGreaterThan(0);
  for (const f of fonts) {
    expect(f.duration).toBeGreaterThanOrEqual(4000);
    expect(f.state).toBe('paused');
  }
}

/** Commandes reçues par la coque simulée : `[nom, arguments]`. */
export type Calls = Array<[string, Record<string, unknown> | null]>;

/**
 * Coque Tauri simulée (`window.__TAURI__`), posée avant le chargement : les
 * commandes sont enregistrées dans `window.__calls`, `window.__emit(nom,
 * charge)` émet un événement. `responses` fixe le retour de commandes.
 */
export async function mockTauri(page: Page, responses: Record<string, unknown> = {}): Promise<void> {
  await page.addInitScript((responses: Record<string, unknown>) => {
    const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
    const calls: Array<[string, unknown]> = [];
    const defaults: Record<string, unknown> = {
      etat_session: { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false },
      etat_enregistrement: { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null },
      historique: [],
    };
    const w = window as unknown as Record<string, unknown>;
    w.__calls = calls;
    w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
    w.__TAURI__ = {
      window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
      core: {
        invoke: async (cmd: string, args?: unknown) => {
          calls.push([cmd, args ?? null]);
          if (cmd in responses) return responses[cmd];
          return cmd in defaults ? defaults[cmd] : null;
        },
      },
      event: {
        listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
          (listeners[event] ??= []).push(handler);
          return () => {};
        },
      },
    };
  }, responses);
}

/** Commandes reçues par la coque simulée. */
export function calls(page: Page): Promise<Calls> {
  return page.evaluate(() => (window as unknown as { __calls: Calls }).__calls);
}

/** Émet un événement de la coque simulée. */
export async function emit(page: Page, event: string, payload: unknown): Promise<void> {
  await page.evaluate(([event, payload]) => (window as unknown as { __emit(e: string, p: unknown): void }).__emit(event as string, payload), [event, payload] as const);
}
