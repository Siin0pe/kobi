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
 * chaque animation de police est lente et avance par paliers, ce qui borne
 * le nombre de redessins du texte.
 */
export async function expectEconomicalFont(page: Page): Promise<void> {
  const fonts = await page.evaluate(() =>
    document
      .getAnimations()
      .filter((a) => a.playState === 'running')
      .filter((a) => (a.effect as KeyframeEffect).getKeyframes().some((k) => 'fontVariationSettings' in k))
      .map((a) => {
        const effect = a.effect as KeyframeEffect;
        return {
          duration: Number(effect.getTiming().duration),
          stepped: effect.getKeyframes().every((k, i, all) => i === all.length - 1 || String(k.easing).startsWith('steps')),
        };
      }),
  );
  expect(fonts.length).toBeGreaterThan(0);
  for (const f of fonts) {
    expect(f.duration).toBeGreaterThanOrEqual(4000);
    expect(f.stepped).toBe(true);
  }
}
