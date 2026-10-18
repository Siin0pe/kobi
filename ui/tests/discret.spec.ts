import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error', 'work'] as const;

test('mode discret : pastille de 32 px colorée selon l’état', async ({ page }) => {
  const errors = await openUi(page, 'discret=1');
  const dot = page.getByRole('button', { name: /^Kobi : Veille/ });
  await expect(dot).toBeVisible();
  await expect(page.locator('.scene')).toBeHidden();
  const box = await dot.boundingBox();
  expect(box?.width).toBe(32);
  expect(box?.height).toBe(32);

  // Sans la transition de couleur (0,3 s) : sinon, sous charge, la couleur lue
  // juste après le changement d'état est encore celle de l'état précédent.
  await page.addStyleTag({ content: '.dot { transition: none !important; }' });
  const colors = new Set<string>();
  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await expect(page.locator('.dot')).toHaveAttribute('data-state', state);
    colors.add(await page.locator('.dot').evaluate((n) => getComputedStyle(n).backgroundColor));
  }
  expect(colors.size).toBe(STATES.length);
  await expectNoErrors(errors);
});

test('un clic sur la pastille déplie la mascotte dans son état courant', async ({ page }) => {
  const errors = await openUi(page, 'discret=1&state=answer');
  const changes: boolean[] = [];
  await page.exposeFunction('compactChanged', (c: boolean) => changes.push(c));
  await page.evaluate(() => (window.kobi.stage.onCompactChange = (c) => (window as unknown as { compactChanged(c: boolean): void }).compactChanged(c)));
  // La pastille pulse en Répond : jamais « immobile », d'où le clic forcé.
  await page.locator('.dot').click({ force: true });
  await expect(page.locator('.scene')).toBeVisible();
  await expect(page.locator('.dot')).toBeHidden();
  await expect(page.locator('.kobi')).toHaveAttribute('data-state', 'answer');
  await expect(page.locator('[data-card=answer]')).toBeVisible();
  expect(changes).toEqual([false]);
  await expectNoErrors(errors);
});

test('repliée, la mascotte ne coûte rien : seule la pastille peut s’animer', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, 'state=think');
  await page.evaluate(() => window.kobi.setCompact(true));
  await page.clock.runFor(2000);
  const running = await page.evaluate(() =>
    document.getAnimations().filter((a) => a.playState === 'running').map((a) => ((a.effect as KeyframeEffect).target as Element).className),
  );
  expect(running).toEqual(['dot']);
  await page.evaluate(() => window.kobi.setState('idle'));
  await page.clock.runFor(1000);
  // Laisse finir la transition de couleur (0,3 s, horloge réelle).
  await page.waitForTimeout(500);
  expect(await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  await expectNoErrors(errors);
});
