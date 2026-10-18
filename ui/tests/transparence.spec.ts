// Règle du propriétaire : la fenêtre de la mascotte reste transparente, jamais
// de fond (pas même pour filmer). Seuls les paramètres de développement
// `backdrop=…` peignent un bureau de test derrière Kobi.
import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

test('la fenêtre de Kobi n’a aucun fond : page, scène et racine transparentes', async ({ page }) => {
  const errors = await openUi(page, '');
  for (const state of ['idle', 'listen', 'answer', 'write'] as const) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    const fonds = await page.evaluate(() =>
      ['html', 'body', '#app', '.stage', '.scene'].map((sel) => {
        const n = document.querySelector(sel);
        if (!n) return { sel, absent: true };
        const st = getComputedStyle(n);
        return { sel, couleur: st.backgroundColor, image: st.backgroundImage };
      }),
    );
    for (const f of fonds) {
      if ('absent' in f) continue;
      expect(f.couleur, `${f.sel} (${state})`).toBe('rgba(0, 0, 0, 0)');
      expect(f.image, `${f.sel} (${state})`).toBe('none');
    }
  }
  await expectNoErrors(errors);
});
