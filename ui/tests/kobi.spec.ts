import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'] as const;
const LOOKS: Record<(typeof STATES)[number], { eye: string; word: string }> = {
  idle: { eye: '–', word: 'zzz' },
  listen: { eye: 'O', word: 'oui ?' },
  think: { eye: '@', word: 'hmm…' },
  write: { eye: 'o', word: 'noté' },
  answer: { eye: '^', word: 'alors' },
  suggest: { eye: '*', word: 'idée !' },
  done: { eye: '^', word: 'fait !' },
  error: { eye: 'x', word: 'oups' },
};

/** Matrice visuelle de la pilule : enveloppe d'inertie × animation de la pilule. */
function pose(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const m = (n: Element) => {
      const t = getComputedStyle(n).transform;
      return t && t !== 'none' ? new DOMMatrix(t) : new DOMMatrix();
    };
    const total = m(document.querySelector('.kobi .inertia')!).multiply(m(document.querySelector('.kobi .pill')!));
    return [total.a, total.b, total.c, total.d, total.e, total.f];
  });
}

test('Kobi passe les 8 états sans erreur', async ({ page }) => {
  const errors = await openUi(page, 'backdrop=1');
  const kobi = page.locator('.kobi');
  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await expect(kobi).toHaveAttribute('data-state', state);
    await expect(kobi).toHaveClass(new RegExp(`m-${state}`));
    await expect(page.locator('.kobi .word')).toHaveText(LOOKS[state].word);
    await expect(page.locator('.kobi .g').first()).toHaveText(LOOKS[state].eye);
    await page.waitForTimeout(300);
  }
  await expectNoErrors(errors);
});

test('les symboles orbitent seulement en Réfléchit', async ({ page }) => {
  const errors = await openUi(page, 'state=think');
  await expect(page.locator('.kobi .orbit')).toBeVisible();
  expect(await page.locator('.kobi .orbit span').evaluateAll((n) => n.map((s) => s.getAnimations().length))).toEqual([1, 1, 1, 1, 1, 1]);
  await page.evaluate(() => window.kobi.setState('answer'));
  await expect(page.locator('.kobi .orbit')).toBeHidden();
  expect(await page.locator('.kobi .orbit span').evaluateAll((n) => n.flatMap((s) => s.getAnimations()).length)).toBe(0);
  await expectNoErrors(errors);
});

test('le changement d’état ne fait pas sauter la pilule', async ({ page }) => {
  const errors = await openUi(page, 'state=think');
  // Réfléchit se balance de ±3° : on change d'état en pleine inclinaison.
  for (const next of ['suggest', 'error', 'idle', 'write'] as const) {
    await page.waitForTimeout(650);
    const [before, after] = await page.evaluate((s) => {
      const m = (n: Element) => {
        const t = getComputedStyle(n).transform;
        return t && t !== 'none' ? new DOMMatrix(t) : new DOMMatrix();
      };
      const read = () =>
        m(document.querySelector('.kobi .inertia')!).multiply(m(document.querySelector('.kobi .pill')!));
      const b = read();
      window.kobi.setState(s, window.kobi.demoData(s));
      const a = read();
      return [[b.a, b.b, b.c, b.d, b.e, b.f], [a.a, a.b, a.c, a.d, a.e, a.f]];
    }, next);
    before.forEach((v, i) => expect(Math.abs(v - after[i]!), `${next}, composante ${i}`).toBeLessThan(0.05));
  }
  expect((await pose(page)).length).toBe(6);
  await expectNoErrors(errors);
});

test('la bouche parle seulement en Répond', async ({ page }) => {
  const errors = await openUi(page, 'state=answer');
  const seen = new Set<string>();
  for (let i = 0; i < 12; i++) {
    seen.add((await page.locator('.kobi .mouth').textContent()) ?? '');
    await page.waitForTimeout(60);
  }
  expect(seen.size).toBeGreaterThan(1);
  await page.evaluate(() => window.kobi.setState('error'));
  await expect(page.locator('.kobi .mouth')).toHaveText('~');
  await page.evaluate(() => window.kobi.setState('idle'));
  await expect(page.locator('.kobi .mouth')).toHaveText('');
  await expectNoErrors(errors);
});

test('les polices viennent du paquet, jamais d’Internet', async ({ page }) => {
  const external: string[] = [];
  page.on('request', (r) => {
    if (!r.url().startsWith('http://127.0.0.1')) external.push(r.url());
  });
  const errors = await openUi(page, 'state=think');
  expect(await page.evaluate(() => document.fonts.check('84px Recursive'))).toBe(true);
  expect(external).toEqual([]);
  await expectNoErrors(errors);
});
