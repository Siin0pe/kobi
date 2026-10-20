// La mascotte en anglais (KOB-209) : avec `ui.language = en`, chaque état et
// chaque bulle de Kobi est en anglais (hors données de démonstration) ; un
// changement de langue venu de la coque les retraduit à chaud.

import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, mockTauri, openUi } from './helpers';

/** Mots et lettres qui trahissent un texte resté en français. */
const FRANCAIS = /[éèêàùçô«»]|\b(le|la|les|des|du|une|vous|votre|aucun|aucune|réunion|pas|est|avec|pour)\b/i;

/** Textes et attributs visibles de la scène, hors données de démo de l'état. */
function textesDeLaScene(page: Page, state: string): Promise<string[]> {
  return page.evaluate((state) => {
    const demo = JSON.stringify(window.kobi.demoData(state as never));
    const textes: string[] = [];
    const racine = document.body;
    const marche = document.createTreeWalker(racine, NodeFilter.SHOW_TEXT);
    for (let n = marche.nextNode(); n; n = marche.nextNode()) {
      const p = n.parentElement;
      if (!p || p.closest('[hidden], script, style') || !p.getClientRects().length) continue;
      const x = n.textContent!.trim();
      if (x) textes.push(x);
    }
    for (const e of racine.querySelectorAll('[aria-label], [title], [placeholder]'))
      for (const a of ['aria-label', 'title', 'placeholder']) {
        const v = e.getAttribute(a)?.trim();
        if (v) textes.push(v);
      }
    return textes.filter((x) => !demo.includes(JSON.stringify(x).slice(1, -1)));
  }, state);
}

test('langue (KOB-209) : en anglais, chaque état de Kobi et sa bulle n’ont plus de français', async ({ page }) => {
  await mockTauri(page, { langue_ui: 'en' });
  const errors = await openUi(page, 'backdrop=1&reduced=1');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  const restes: string[] = [];
  for (const state of await page.evaluate(() => [...window.kobi.states])) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await expect(page.locator('.kobi')).toHaveAttribute('data-state', state);
    await page.waitForTimeout(300);
    for (const x of await textesDeLaScene(page, state)) if (FRANCAIS.test(x)) restes.push(`${state} : ${x}`);
  }
  expect([...new Set(restes)]).toEqual([]);
  await expectNoErrors(errors);
});

test('langue (KOB-209) : réunion, menu et bulle d’écriture en anglais', async ({ page }) => {
  await mockTauri(page, { langue_ui: 'en' });
  const errors = await openUi(page, 'backdrop=1&reduced=1&reunion=Weekly');
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now() - 60_000, notes: 2, titre: 'Weekly' } as never));
  await page.evaluate(() => window.kobi.ecrire());
  await page.waitForTimeout(300);
  const restes = (await textesDeLaScene(page, 'idle')).filter((x) => FRANCAIS.test(x));
  expect(restes).toEqual([]);
  await expectNoErrors(errors);
});

test('langue (KOB-209) : le mot de Kobi et sa bulle changent de langue à chaud', async ({ page }) => {
  await mockTauri(page, { langue_ui: 'fr' });
  const errors = await openUi(page, 'backdrop=1&reduced=1');
  await page.evaluate(() => window.kobi.setState('listen', window.kobi.demoData('listen')));
  const mot = page.locator('.kobi .word');
  await expect(mot).toHaveText('j’écoute…');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('langue', 'en'));
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(mot).not.toHaveText('j’écoute…');
  const enAnglais = await mot.textContent();
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('langue', 'fr'));
  await expect(mot).toHaveText('j’écoute…');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('langue', 'en'));
  await expect(mot).toHaveText(enAnglais!);
  await expectNoErrors(errors);
});
