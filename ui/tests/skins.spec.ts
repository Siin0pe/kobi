// Contrat vérifié pour chaque skin enregistré : 8 états sans erreur, pause
// complète fenêtre masquée, rien ne bouge avec « réduire les animations »,
// aucune ressource chargée hors de l'application.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const SKINS = ['kobi', 'gribouille', 'mercure'];
const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'] as const;

/**
 * Animations continues en cours (CSS, Web Animations, SMIL non pausé). Les
 * transitions CSS, brèves et finies, ne comptent pas.
 */
function moving(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      document.getAnimations().filter((a) => a.playState === 'running' && !(a instanceof CSSTransition)).length +
      [...document.querySelectorAll('svg')].filter((s) => s.querySelector('animate, animateTransform, animateMotion') && !s.animationsPaused()).length,
  );
}

for (const skin of SKINS) {
  test.describe(`skin ${skin}`, () => {
    test('passe les 8 états sans erreur ni requête externe', async ({ page }) => {
      const external: string[] = [];
      page.on('request', (r) => {
        if (!r.url().startsWith('http://127.0.0.1')) external.push(r.url());
      });
      const errors = await openUi(page, `skin=${skin}&backdrop=1`);
      await expect(page.locator('.stage')).toHaveAttribute('data-skin', skin);
      for (const state of STATES) {
        await page.evaluate((s) => window.kobi.play(s), state);
        await expect(page.locator(`.skin-${skin}`)).toHaveAttribute('data-state', state);
        await page.waitForTimeout(250);
      }
      expect(external).toEqual([]);
      await expectNoErrors(errors);
    });

    test('réduire les animations : rien ne bouge', async ({ page }) => {
      await page.emulateMedia({ reducedMotion: 'reduce' });
      const errors = await openUi(page, `skin=${skin}`);
      for (const state of STATES) {
        await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
        await page.waitForTimeout(50);
        expect(await moving(page), state).toBe(0);
      }
      await expectNoErrors(errors);
    });

    test('fenêtre masquée : tout s’arrête, puis repart', async ({ page }) => {
      const errors = await openUi(page, `skin=${skin}&state=think`);
      await page.waitForTimeout(300);
      expect(await moving(page)).toBeGreaterThan(0);
      const setHidden = (hidden: boolean) =>
        page.evaluate((hidden) => {
          Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (hidden ? 'hidden' : 'visible') });
          document.dispatchEvent(new Event('visibilitychange'));
        }, hidden);
      await setHidden(true);
      expect(await moving(page)).toBe(0);
      await setHidden(false);
      expect(await moving(page)).toBeGreaterThan(0);
      await expectNoErrors(errors);
    });
  });
}
