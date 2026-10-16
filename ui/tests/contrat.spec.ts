import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'] as const;
const CARDS: Record<(typeof STATES)[number], string | null> = {
  idle: null,
  listen: 'question',
  think: 'sources',
  write: 'notes',
  answer: 'answer',
  suggest: 'action',
  done: 'action',
  error: 'error',
};

test('le skin factice passe les 8 états sans erreur', async ({ page }) => {
  const errors = await openUi(page, 'skin=factice&backdrop=1');
  const stage = page.locator('.stage');
  const skin = page.locator('.skin-factice');
  await expect(stage).toHaveAttribute('data-skin', 'factice');

  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await expect(stage).toHaveAttribute('data-state', state);
    await expect(skin).toHaveAttribute('data-state', state);
    await expect(page.locator('.scene')).toHaveAttribute('aria-label', /^Kobi : /);
    await expect(page.locator('[role=status]')).not.toBeEmpty();
    const card = CARDS[state];
    if (card) await expect(page.locator(`.bubbles [data-card=${card}]`)).toBeVisible();
    else await expect(page.locator('.bubbles [data-card]')).toHaveCount(0);
  }
  await expectNoErrors(errors);
});

test('les données en flux mettent à jour la carte sans la recréer', async ({ page }) => {
  const errors = await openUi(page, 'skin=factice&state=answer');
  const card = page.locator('[data-card=answer]');
  await card.evaluate((n) => n.setAttribute('data-marker', '1'));
  await page.evaluate(() => window.kobi.update({ answer: 'Texte complet de la réponse.' }));
  await expect(card).toHaveAttribute('data-marker', '1');
  await expect(card.locator('.text')).toHaveText('Texte complet de la réponse.');
  await expectNoErrors(errors);
});

test('la carte d’action remonte la décision de l’utilisateur', async ({ page }) => {
  const errors = await openUi(page, 'skin=factice&state=suggest');
  await page.evaluate(() => {
    (window as unknown as { decisions: unknown[] }).decisions = [];
    window.kobi.stage.onAction((d) => (window as unknown as { decisions: unknown[] }).decisions.push(d));
  });
  await page.getByRole('button', { name: 'Créer' }).click();
  const decisions = await page.evaluate(() => (window as unknown as { decisions: unknown[] }).decisions);
  expect(decisions).toEqual([{ id: 'demo-ticket', decision: 'confirm' }]);
  await expectNoErrors(errors);
});

test('un changement de skin garde l’état courant', async ({ page }) => {
  const errors = await openUi(page, 'skin=factice&state=think');
  await page.evaluate(() => window.kobi.useSkin('factice'));
  await expect(page.locator('.skin-factice')).toHaveCount(1);
  await expect(page.locator('.skin-factice')).toHaveAttribute('data-state', 'think');
  await expectNoErrors(errors);
});
