// Deuxième passe des animations (KOB-149) : répertoire de la veille, pincement
// au clic, inertie quand la fenêtre bouge, yeux qui suivent la voix, tête de
// frappe brouillée. Garde-fous : rien ne bouge avec « réduire les animations ».
import { expect, test } from '@playwright/test';
import { emit, expectNoErrors, kobiPose, mockTauri, openUi } from './helpers';

test('la veille a au moins 15 scènes, chacune vit puis rend la main sans laisser de symbole', async ({ page }) => {
  test.setTimeout(120_000);
  const errors = await openUi(page, 'backdrop=1');
  await kobiPose(page);
  const names = await page.evaluate(() => (window.kobi.stage.kobi as unknown as { idleNames(): string[] }).idleNames());
  expect(names.length).toBeGreaterThanOrEqual(15);
  for (const name of names) {
    // Quelque chose bouge : un geste, un symbole, une lettre, un regard (tenus
    // par le métronome après 20 s). Relevé dans la page dès le lancement de la
    // scène : un regard en coin n'est qu'une transition courte, finie avant un
    // aller-retour du test sous charge (KOB-172).
    const { played, idle, moved } = await page.evaluate(async (n) => {
      const kobi = document.querySelector('.kobi')!;
      const played = (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle(n);
      const idle = kobi.getAttribute('data-idle');
      const bouge = () => kobi.getAnimations({ subtree: true }).filter((a) => !(a instanceof CSSAnimation) && (a.playState === 'running' || a.playState === 'paused')).length + kobi.querySelectorAll('.burst span').length;
      const fin = performance.now() + 1500;
      let moved = bouge();
      while (moved === 0 && performance.now() < fin) {
        await new Promise((r) => requestAnimationFrame(r));
        moved = bouge();
      }
      return { played, idle, moved };
    }, name);
    expect(played, name).toBe(true);
    expect(idle, name).toBe(name);
    expect(moved, name).toBeGreaterThan(0);
    // La scène se termine, les symboles sont retirés.
    await expect(page.locator('.kobi')).not.toHaveAttribute('data-idle', name, { timeout: 6000 });
    await expect.poll(() => page.locator('.kobi .burst span').count(), { timeout: 2000, message: name }).toBe(0);
  }
  await expectNoErrors(errors);
});

test('clic sur Kobi : il se fait pincer (écrasé), puis sursaute de surprise', async ({ page }) => {
  // Gestes joués sur le calque .react, relevés dans la page : l'écrasement ne
  // dure que 240 ms, trop peu pour être lu à coup sûr sous charge (KOB-172).
  await page.addInitScript(() => {
    const gestes: string[][] = [];
    (window as unknown as { __gestes: string[][] }).__gestes = gestes;
    const animate = Element.prototype.animate;
    Element.prototype.animate = function (this: Element, ...args: Parameters<Element['animate']>) {
      if (this.classList.contains('react') && Array.isArray(args[0])) gestes.push((args[0] as Keyframe[]).map((k) => String(k.transform ?? '')));
      return animate.apply(this, args);
    };
  });
  const errors = await openUi(page, 'state=listen&backdrop=1');
  await expect(page.locator('.kobi')).toHaveClass(/m-listen/);
  const box = (await page.locator('.kobi .pill').boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.evaluate(() => ((window as unknown as { __gestes: string[][] }).__gestes.length = 0));
  await page.mouse.down();
  await expect(page.locator('.kobi')).toHaveAttribute('data-pinched', '1');
  await expect(page.locator('.kobi .g').first()).toHaveText('>');
  // Écrasé (plus large que haut), puis le sursaut part de la pose écrasée.
  const gestes = () => page.evaluate(() => (window as unknown as { __gestes: string[][] }).__gestes);
  await expect.poll(async () => (await gestes()).some((g) => g.includes('scale(1.2,.72)'))).toBe(true);
  await expect.poll(async () => (await gestes()).some((g) => g.includes('translateY(-26px) scale(.88,1.16) rotate(3deg)'))).toBe(true);
  await page.mouse.up();
  await expect(page.locator('.kobi .g').first()).toHaveText('O');
  await expect(page.locator('.kobi')).not.toHaveAttribute('data-pinched', '1', { timeout: 2000 });
  await expectNoErrors(errors);
});

test('la fenêtre déplacée : Kobi ballotte puis revient au repos (événement tauri://move)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, 'backdrop=1');
  await kobiPose(page);
  const drag = page.locator('.kobi .drag');
  for (let i = 0; i <= 6; i++) await emit(page, 'tauri://move', { x: 100 + i * 25, y: 300 });
  await expect.poll(() => drag.evaluate((n) => n.style.transform)).toMatch(/translate\(-\d/);
  // Le ressort s'amortit et la boucle s'arrête.
  await expect.poll(() => drag.evaluate((n) => n.style.transform), { timeout: 4000 }).toBe('');
  await expectNoErrors(errors);
});

test('en écoute, les yeux grossissent avec la voix', async ({ page }) => {
  const errors = await openUi(page, 'state=listen');
  await kobiPose(page);
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
  // Contrôle négatif : 200 ms laissés à un geste pour apparaître.
  await page.waitForTimeout(200);
  expect(await page.locator('.kobi').getAttribute('data-pinched')).toBeNull();
  expect(await page.locator('.kobi .drag').evaluate((n) => n.style.transform)).toBe('');
  expect(await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  await expectNoErrors(errors);
});

test('fenêtre masquée : aucune scène de veille ne se lance', async ({ page }) => {
  const errors = await openUi(page, '');
  await kobiPose(page);
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
    // Échantillonnage de la tête de frappe toutes les 25 ms.
    await page.waitForTimeout(25);
  }
  expect([...heads].some((c) => '#%&@*?!/<>{}=+$'.includes(c))).toBe(true);
  await expect(text).toHaveText(answer);
  await expectNoErrors(errors);
});

test('repos économe : une scène de la veille se joue à 60 images/s, les boucles de fond restent au métronome (KOB-170)', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  // Économie d'énergie activée (KOB-200) : l'ancien repos économe.
  await page.evaluate(() => window.kobiAnimations.poser({ economie: true }));
  const stage = page.locator('.stage');
  await page.clock.runFor(21_000);
  await expect(page.locator('.kobi')).toHaveClass(/deep/);
  await page.clock.runFor(500);
  await expect(stage).toHaveAttribute('data-fps', '3');
  expect(await page.evaluate(() => (window.kobi.stage.kobi as unknown as { playIdle(n: string): boolean }).playIdle('whistle'))).toBe(true);
  await expect(stage).toHaveAttribute('data-geste', '');
  // Le geste (animations Web de la scène) tourne librement…
  expect(await page.evaluate(() => document.querySelector('.kobi')!.getAnimations({ subtree: true }).filter((a) => !(a instanceof CSSAnimation) && a.playState === 'running').length)).toBeGreaterThan(0);
  // … mais le métronome tient toujours les boucles de fond : l'ondulation de la
  // police ne repart pas à 60 images/s (c'est elle qui coûte, KOB-170).
  await expect(stage).toHaveAttribute('data-fps', '3');
  const vagues = await page.evaluate(() =>
    document.querySelector('.kobi')!.getAnimations({ subtree: true }).filter((a) => a instanceof CSSAnimation && a.animationName === 'kobi-fin').map((a) => a.playState),
  );
  expect(vagues.length).toBeGreaterThan(0);
  expect(new Set(vagues)).toEqual(new Set(['paused']));
  await expect(page.locator('.kobi')).toHaveClass(/deep/);
  await page.clock.runFor(3_000);
  await expect(stage).not.toHaveAttribute('data-geste', '');
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

test('réglage « Animations » changé dans les Réglages de l’app : la mascotte et l’app suivent tout de suite', async ({ page, context }) => {
  const errors = await openUi(page, 'state=think');
  await expect(page.locator('.stage')).not.toHaveClass(/reduced-motion/);
  // La section Réglages de l'app Kobi, autre fenêtre de la même origine.
  const reglages = await context.newPage();
  await reglages.goto('/app.html#reglages/animations');
  await reglages.getByRole('radio', { name: /Réduites/ }).click();
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  // L'app elle-même suit aussi (classe sur <html>, lue par animationsReduites()).
  await expect(reglages.locator('html')).toHaveClass(/reduced-motion/);
  await expect.poll(() => page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  await reglages.getByRole('radio', { name: /Toujours/ }).click();
  await expect(page.locator('.stage')).not.toHaveClass(/reduced-motion/);
  await expect(reglages.locator('html')).not.toHaveClass(/reduced-motion/);
  // « Comme Windows » avec Windows qui réduit : immobile.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await reglages.getByRole('radio', { name: /Comme Windows/ }).click();
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  await expectNoErrors(errors);
});

test('réunion, réglage « toujours » avec Windows réduit : REC et fiche de notes, cliquables, ne bougent pas', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors = await openUi(page, 'backdrop=1');
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 2, titre: 'Point hebdo', derniere_note: 'Budget validé' }));
  await expect(page.locator('.stage')).not.toHaveClass(/reduced-motion/);
  await kobiPose(page);
  const boxes = () =>
    page.evaluate(() =>
      ['.rec', '.card-notes'].map((s) => {
        const r = document.querySelector(s)!.getBoundingClientRect();
        return [r.x, r.y, r.width, r.height].map((v) => Math.round(v * 10) / 10).join(',');
      }),
    );
  const first = await boxes();
  for (let i = 0; i < 8; i++) {
    // Échantillonnage : la position est relevée toutes les 200 ms.
    await page.waitForTimeout(200);
    expect(await boxes()).toEqual(first);
  }
  // La vie passe par les enfants : le surtitre de la fiche et le point rouge.
  const vivants = await page.evaluate(() => ['.card-notes .kicker', '.rec-dot'].map((s) => document.querySelector(s)!.getAnimations().filter((a) => a.playState === 'running').length));
  expect(vivants.every((n) => n > 0)).toBe(true);
  await expectNoErrors(errors);
});

test('fenêtre masquée pendant que le regard est tourné : le regard recentré ne tourne plus', async ({ page }) => {
  const errors = await openUi(page, 'state=listen');
  await kobiPose(page);
  await page.mouse.move(805, 30);
  await expect.poll(() => page.locator('.kobi .gaze').evaluate((n) => n.style.transform)).toMatch(/translate\(1\d|translate\(2\d/);
  // Voulu : le regard reste tourné un moment avant que la fenêtre soit masquée.
  await page.waitForTimeout(500);
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  expect(await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').map((a) => a.constructor.name + ':' + ((a as CSSTransition).transitionProperty ?? (a as CSSAnimation).animationName ?? '')))).toEqual([]);
  await expectNoErrors(errors);
});
