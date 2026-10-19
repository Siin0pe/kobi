import { expect, type Locator, type Page } from '@playwright/test';

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
 * Repos économe : Kobi vit encore (il n'est jamais figé), mais ses boucles
 * (respiration, griffonnage…) sont tenues par le métronome de la scène
 * (econome.ts) au lieu de produire leurs propres images. Au repos, plus de
 * mot sous la tête (KOB-204) : c'est la pilule qui vit.
 */
export async function expectEconomicalLoops(page: Page): Promise<void> {
  const loops = await page.evaluate(() =>
    document
      .querySelector('.kobi')!
      .getAnimations({ subtree: true })
      .filter((a) => a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity)
      .map((a) => ({ name: (a as CSSAnimation).animationName, state: a.playState })),
  );
  expect(loops.length).toBeGreaterThan(0);
  for (const l of loops) expect(l.state, l.name).toBe('paused');
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

/**
 * Kobi posé (KOB-172) : la classe de l'état affiché est en place et plus aucun
 * geste, morphing des yeux ni inertie ne tourne. Remplace les attentes fixes
 * après l'ouverture ou un changement d'état, qui ne suffisaient pas sous charge.
 */
export async function kobiPose(page: Page): Promise<void> {
  await page.waitForFunction(() => {
    const kobi = document.querySelector('.kobi');
    const state = kobi?.getAttribute('data-state');
    if (!kobi || !state || !kobi.classList.contains(`m-${state}`)) return false;
    return ![...kobi.querySelectorAll('.react, .inertia, .eyes')].some((l) => l.getAnimations().some((a) => !(a instanceof CSSAnimation) && a.playState === 'running'));
  });
}

/** Plus aucune animation qui tourne sur l'élément (et ses enfants si `subtree`) : il a fini d'arriver. */
export async function sansAnimation(locator: Locator, subtree = false): Promise<void> {
  await expect.poll(() => locator.evaluate((n, s) => n.getAnimations({ subtree: s }).filter((a) => a.playState === 'running').length, subtree)).toBe(0);
}
