// Deuxième passe des animations (KOB-149) : répertoire de la veille, pincement
// au clic, inertie quand la fenêtre bouge, yeux qui suivent la voix, tête de
// frappe brouillée. Garde-fous : rien ne bouge avec « réduire les animations ».
import { expect, test } from '@playwright/test';
import { emit, expectNoErrors, mockTauri, openUi } from './helpers';

test('la veille a au moins 15 scènes, chacune vit puis rend la main sans laisser de symbole', async ({ page }) => {
  test.setTimeout(120_000);
  const errors = await openUi(page, 'backdrop=1');
  await page.waitForTimeout(800);
  const names = await page.evaluate(() => (window.kobi.stage.kobi as unknown as { idleNames(): string[] }).idleNames());
  expect(names.length).toBeGreaterThanOrEqual(15);
  for (const name of names) {
    const played = await page.evaluate((n) => (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle(n), name);
    expect(played, name).toBe(true);
    await expect(page.locator('.kobi')).toHaveAttribute('data-idle', name);
    // Quelque chose bouge : un geste, un symbole, une lettre (tenus par le métronome après 20 s).
    await expect
      .poll(() => page.evaluate(() => document.querySelector('.kobi')!.getAnimations({ subtree: true }).filter((a) => !(a instanceof CSSAnimation) && (a.playState === 'running' || a.playState === 'paused')).length + document.querySelectorAll('.kobi .burst span').length), { timeout: 1500, message: name })
      .toBeGreaterThan(0);
    // La scène se termine, les symboles sont retirés.
    await expect(page.locator('.kobi')).not.toHaveAttribute('data-idle', name, { timeout: 6000 });
    await expect.poll(() => page.locator('.kobi .burst span').count(), { timeout: 2000, message: name }).toBe(0);
  }
  await expectNoErrors(errors);
});

test('clic sur Kobi : il se fait pincer (écrasé), puis sursaute de surprise', async ({ page }) => {
  const errors = await openUi(page, 'state=listen&backdrop=1');
  await page.waitForTimeout(900);
  const box = (await page.locator('.kobi .pill').boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.waitForTimeout(700);
  await page.mouse.down();
  await expect(page.locator('.kobi')).toHaveAttribute('data-pinched', '1');
  await expect(page.locator('.kobi .g').first()).toHaveText('>');
  // Écrasé : plus large que haut.
  await expect.poll(() => page.locator('.kobi .react').evaluate((n) => new DOMMatrix(getComputedStyle(n).transform).a)).toBeGreaterThan(1.05);
  await page.mouse.up();
  await expect(page.locator('.kobi .g').first()).toHaveText('O');
  await expect(page.locator('.kobi')).not.toHaveAttribute('data-pinched', '1', { timeout: 2000 });
  await expectNoErrors(errors);
});

test('la fenêtre déplacée : Kobi ballotte puis revient au repos (événement tauri://move)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, 'backdrop=1');
  await page.waitForTimeout(600);
  const drag = page.locator('.kobi .drag');
  for (let i = 0; i <= 6; i++) await emit(page, 'tauri://move', { x: 100 + i * 25, y: 300 });
  await expect.poll(() => drag.evaluate((n) => n.style.transform)).toMatch(/translate\(-\d/);
  // Le ressort s'amortit et la boucle s'arrête.
  await expect.poll(() => drag.evaluate((n) => n.style.transform), { timeout: 4000 }).toBe('');
  await expectNoErrors(errors);
});

test('en écoute, les yeux grossissent avec la voix', async ({ page }) => {
  const errors = await openUi(page, 'state=listen');
  await page.waitForTimeout(700);
  const eyes = page.locator('.kobi .eyes');
  expect(await eyes.evaluate((n) => getComputedStyle(n).scale)).toMatch(/^(none|1)$/);
  await page.evaluate(() => window.kobi.setLevel(1));
  await expect.poll(() => eyes.evaluate((n) => parseFloat(getComputedStyle(n).scale))).toBeGreaterThan(1.05);
  await expectNoErrors(errors);
});

test('réduire les animations : ni scène de veille, ni pincement, ni ballottement', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, 'reduced=1&backdrop=1');
  expect(await page.evaluate(() => (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle('juggle'))).toBe(false);
  const box = (await page.locator('.kobi .pill').boundingBox())!;
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  for (let i = 0; i <= 4; i++) await emit(page, 'tauri://move', { x: 100 + i * 40, y: 300 });
  await page.waitForTimeout(200);
  expect(await page.locator('.kobi').getAttribute('data-pinched')).toBeNull();
  expect(await page.locator('.kobi .drag').evaluate((n) => n.style.transform)).toBe('');
  expect(await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  await expectNoErrors(errors);
});

test('fenêtre masquée : aucune scène de veille ne se lance', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.waitForTimeout(500);
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  expect(await page.evaluate(() => (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle('whistle'))).toBe(false);
  await expectNoErrors(errors);
});

test('la frappe passe par un symbole brouillé en tête, puis le texte est exact', async ({ page }) => {
  const errors = await openUi(page, 'backdrop=1');
  const answer = 'Le fichier refresh.ts appelle la route /auth/refresh toutes les 15 minutes.';
  await page.evaluate((a) => window.kobi.setState('answer', { question: 'Où est le rafraîchissement ?', answer: a }), answer);
  const text = page.locator('[data-card=answer] .text');
  // Pendant la frappe, le dernier caractère affiché est souvent un symbole.
  const heads = new Set<string>();
  for (let i = 0; i < 40; i++) {
    const t = (await text.textContent()) ?? '';
    if (t.length > 0 && t.length < answer.length) heads.add(t.slice(-1));
    await page.waitForTimeout(25);
  }
  expect([...heads].some((c) => '#%&@*?!/<>{}=+$'.includes(c))).toBe(true);
  await expect(text).toHaveText(answer);
  await expectNoErrors(errors);
});

test('repos économe : une scène de la veille se joue à 60 images/s, puis le métronome reprend', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  const stage = page.locator('.stage');
  await page.clock.runFor(21_000);
  await expect(page.locator('.kobi')).toHaveClass(/deep/);
  await page.clock.runFor(500);
  await expect(stage).toHaveAttribute('data-fps', '3');
  expect(await page.evaluate(() => (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle('whistle'))).toBe(true);
  await expect(stage).toHaveAttribute('data-fps', '0');
  // Toujours au repos économe : le mot garde son ondulation lente.
  await expect(page.locator('.kobi')).toHaveClass(/deep/);
  await page.clock.runFor(3_000);
  await expect(stage).toHaveAttribute('data-fps', '3');
  await expectNoErrors(errors);
});

test('réglage « Animations » : par défaut Kobi bouge même si Windows réduit les animations', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors = await openUi(page, 'state=think');
  await expect(page.locator('.stage')).not.toHaveClass(/reduced-motion/);
  await expect.poll(() => page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBeGreaterThan(0);
  await expectNoErrors(errors);
});

test('réglage « Animations » changé dans une autre fenêtre : la mascotte suit tout de suite', async ({ page, context }) => {
  const errors = await openUi(page, 'state=think');
  await expect(page.locator('.stage')).not.toHaveClass(/reduced-motion/);
  // Les Réglages, autre fenêtre de la même app (même origine).
  const reglages = await context.newPage();
  await reglages.goto('/reglages.html?panneau=animations');
  await reglages.getByRole('radio', { name: /Réduites/ }).click();
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  await expect.poll(() => page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  await reglages.getByRole('radio', { name: /Toujours/ }).click();
  await expect(page.locator('.stage')).not.toHaveClass(/reduced-motion/);
  // « Comme Windows » avec Windows qui réduit : immobile.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await reglages.getByRole('radio', { name: /Comme Windows/ }).click();
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  await expectNoErrors(errors);
});
