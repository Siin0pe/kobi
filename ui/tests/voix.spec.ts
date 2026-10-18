// Réponse à voix haute ou en texte (KOB-142) : la scène porte l'état de la
// voix envoyé par la coque (`etat_voix_ui`, événement `voix`).
import { expect, test } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

test('la scène suit le réglage et la parole de Kobi ; Échap coupe la voix', async ({ page }) => {
  await mockTauri(page, { etat_voix_ui: { quotidien: true, reunion: false, actif: true, parle: false } });
  const errors = await openUi(page, '');
  const stage = page.locator('.stage');
  await expect(stage).toHaveAttribute('data-voix', 'active');
  await emit(page, 'voix', { quotidien: true, reunion: false, actif: true, parle: true });
  await expect(stage).toHaveAttribute('data-voix', 'parle');

  // Échap pendant une réponse : la commande `interrompre` coupe aussi la voix.
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await calls(page)).map(([c]) => c)).toContain('interrompre');

  await emit(page, 'voix', { quotidien: false, reunion: false, actif: false, parle: false });
  await expect(stage).toHaveAttribute('data-voix', 'texte');
  await expectNoErrors(errors);
});
