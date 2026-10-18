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

test('chaque état affiche sa carte, sans erreur ni requête externe', async ({ page }) => {
  const external: string[] = [];
  page.on('request', (r) => {
    if (!r.url().startsWith('http://127.0.0.1')) external.push(r.url());
  });
  const errors = await openUi(page, 'backdrop=1');
  const stage = page.locator('.stage');
  const kobi = page.locator('.kobi');

  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await expect(stage).toHaveAttribute('data-state', state);
    await expect(kobi).toHaveAttribute('data-state', state);
    await expect(page.locator('.scene')).toHaveAttribute('aria-label', /^Kobi : /);
    await expect(page.locator('[role=status]')).not.toBeEmpty();
    const card = CARDS[state];
    if (card) await expect(page.locator(`.bubbles [data-card=${card}]`)).toBeVisible();
    else await expect(page.locator('.bubbles [data-card]')).toHaveCount(0);
  }
  expect(external).toEqual([]);
  await expectNoErrors(errors);
});

test('les données en flux mettent à jour la carte sans la recréer', async ({ page }) => {
  const errors = await openUi(page, 'state=answer');
  const card = page.locator('[data-card=answer]');
  await card.evaluate((n) => n.setAttribute('data-marker', '1'));
  await page.evaluate(() => window.kobi.update({ answer: 'Texte complet de la réponse.' }));
  await expect(card).toHaveAttribute('data-marker', '1');
  await expect(card.locator('.text')).toHaveText('Texte complet de la réponse.');
  await expectNoErrors(errors);
});

test('la carte d’action remonte la décision de l’utilisateur', async ({ page }) => {
  const errors = await openUi(page, 'state=suggest');
  await page.evaluate(() => {
    (window as unknown as { decisions: unknown[] }).decisions = [];
    window.kobi.stage.onAction((d) => (window as unknown as { decisions: unknown[] }).decisions.push(d));
  });
  await page.getByRole('button', { name: 'Créer' }).click();
  const decisions = await page.evaluate(() => (window as unknown as { decisions: unknown[] }).decisions);
  expect(decisions).toEqual([{ id: 'demo-ticket', decision: 'confirm' }]);
  await expectNoErrors(errors);
});

test('les cartes à boutons vivent par leurs enfants décoratifs, les boutons ne bougent jamais', async ({ page }) => {
  const errors = await openUi(page, 'state=suggest');
  const card = page.locator('[data-card=action]');
  await expect(card).toBeVisible();
  await page.waitForTimeout(500);
  // La carte a fini d'arriver : plus aucune animation sur elle ni sur ses boutons.
  expect(await card.evaluate((n) => n.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  const live = await card.evaluate((n) => n.getAnimations({ subtree: true }).filter((a) => a.playState === 'running').length);
  expect(live).toBeGreaterThanOrEqual(3);
  const button = card.getByRole('button', { name: 'Créer' });
  const boxes = [];
  for (let i = 0; i < 6; i++) {
    boxes.push(await button.boundingBox());
    await page.waitForTimeout(250);
  }
  for (const b of boxes) expect(b).toEqual(boxes[0]);
  await expectNoErrors(errors);
});
