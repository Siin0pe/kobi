// « Kobi réfléchit » (KOB-241) : trois propositions, chacune en CSS seul
// (transform et opacité), sans graisse animée ni boucle JavaScript, et au pas
// d'« Images par seconde ».
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const PROPRES: Record<string, string> = { a: '.roue', b: '.points', c: '.pensee' };

async function choisir(page: Page, v: string): Promise<string[]> {
  await page.addInitScript((v) => localStorage.setItem('kobi.reflexion', v), v);
  return openUi(page, '');
}

const etat = (page: Page, s: string) => page.evaluate((s) => window.kobi.setState(s as never, window.kobi.demoData(s as never)), s);

for (const v of ['a', 'b', 'c']) {
  test(`proposition ${v.toUpperCase()} : visible seulement en Réfléchit, transform et opacité seulement`, async ({ page }) => {
    const errors = await choisir(page, v);
    const propre = page.locator(`.kobi ${PROPRES[v]}`).first();
    await expect(page.locator('.kobi')).toHaveAttribute('data-reflexion', v);
    await expect(propre).toBeHidden();
    await etat(page, 'listen');
    await etat(page, 'think');
    await expect(propre).toBeVisible();
    await page.waitForTimeout(800);
    // Rien qui remette en page à chaque image : ni graisse, ni taille, ni position.
    const proprietes = await page.evaluate(() =>
      document
        .querySelector('.kobi .reflexion')!
        .getAnimations({ subtree: true })
        .filter((a) => a.playState === 'running')
        .flatMap((a) => (a.effect as KeyframeEffect).getKeyframes().flatMap((k) => Object.keys(k)))
        .filter((k) => !['offset', 'easing', 'composite', 'computedOffset', 'transform', 'opacity', 'translate', 'scale', 'rotate'].includes(k)),
    );
    expect(proprietes).toEqual([]);
    // Les autres propositions restent cachées.
    for (const [autre, sel] of Object.entries(PROPRES)) if (autre !== v) await expect(page.locator(`.kobi ${sel}`).first()).toBeHidden();
    await etat(page, 'answer');
    await expect(propre).toBeHidden();
    await expectNoErrors(errors);
  });

  test(`proposition ${v.toUpperCase()} : à 15 images/s, tout est en escalier`, async ({ page }) => {
    const errors = await choisir(page, v);
    await page.evaluate(() => window.kobiAnimations.ecran(60));
    await page.evaluate(() => window.kobiAnimations.poser({ fps: 15 }));
    await etat(page, 'think');
    await page.waitForTimeout(800);
    const hors = await page.evaluate(() =>
      document
        .getAnimations()
        .filter((a) => a.playState === 'running' && !String(a.effect?.getTiming().easing).startsWith('steps('))
        .map((a) => (a as CSSAnimation).animationName ?? 'WAAPI'),
    );
    expect(hors).toEqual([]);
    await expectNoErrors(errors);
  });
}
