// Animations de Kobi (KOB-137) : transitions sans saut, vie de chaque état,
// curseur, volume du micro, bouche calée sur le flux, enregistrement.
import { expect, test } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'] as const;

test('aucune image ne saute pendant les transitions entre les 8 états', async ({ page }) => {
  test.setTimeout(90_000);
  const errors = await openUi(page, 'backdrop=1');
  const pairs: Array<[string, string]> = [];
  for (const a of STATES) for (const b of STATES) if (a !== b) pairs.push([a, b]);
  const worst = await page.evaluate(async (pairs) => {
    const pill = document.querySelector('.kobi .pill')!;
    const box = () => {
      const r = pill.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
    };
    let worst = { jump: 0, speed: 0, pair: '' };
    for (const [a, b] of pairs) {
      window.kobi.setState(a as never, window.kobi.demoData(a as never));
      await new Promise((r) => setTimeout(r, 450));
      // Déplacement de chaque image, ramené à une image de 16,7 ms.
      const moves: number[] = [];
      let last = box();
      let lastT = performance.now();
      window.kobi.setState(b as never, window.kobi.demoData(b as never));
      const end = lastT + 700;
      await new Promise<void>((resolve) => {
        const tick = (t: number) => {
          const now = box();
          const frames = Math.max(1, (t - lastT) / 16.7);
          moves.push(Math.max(Math.hypot(now.x - last.x, now.y - last.y), Math.abs(now.w - last.w), Math.abs(now.h - last.h)) / frames);
          last = now;
          lastT = t;
          if (t < end) requestAnimationFrame(tick);
          else resolve();
        };
        requestAnimationFrame(tick);
      });
      // Un saut est une image qui bouge bien plus que ses deux voisines ; un
      // mouvement rapide mais continu accélère et freine sur plusieurs images.
      for (let i = 1; i < moves.length - 1; i++) {
        const jump = moves[i]! - Math.max(moves[i - 1]!, moves[i + 1]!);
        if (jump > worst.jump) worst = { ...worst, jump, pair: `${a} → ${b}` };
        if (moves[i]! > worst.speed) worst.speed = moves[i]!;
      }
    }
    return worst;
  }, pairs);
  expect(worst.jump, `saut : ${worst.pair}`).toBeLessThan(9);
  expect(worst.speed).toBeLessThan(30);
  await expectNoErrors(errors);
});

test('chaque transition est chorégraphiée : anticipation, yeux en fente, puis décollage', async ({ page }) => {
  const errors = await openUi(page, 'state=listen');
  await page.waitForTimeout(700);
  await page.evaluate(() => window.kobi.setState('suggest', window.kobi.demoData('suggest')));
  // Pendant l'anticipation, l'état est déjà annoncé mais la classe de l'ancien reste posée.
  await expect(page.locator('.kobi')).toHaveAttribute('data-state', 'suggest');
  expect(await page.locator('.kobi').getAttribute('class')).toContain('m-listen');
  // Le geste (calque .react) et le morphing des yeux tournent.
  expect(await page.locator('.kobi .react').evaluate((n) => n.getAnimations().length)).toBe(1);
  expect(await page.locator('.kobi .eyes').evaluate((n) => n.getAnimations().length)).toBe(1);
  await expect(page.locator('.kobi')).toHaveClass(/m-suggest/);
  // Le « ! » surgit.
  await expect(page.locator('.kobi .bang')).toBeVisible();
  await expectNoErrors(errors);
});

test('la bouche suit la réponse en flux et se tait quand le flux s’arrête', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('answer', { answer: '' }));
  await page.clock.runFor(1_000);
  const mouth = page.locator('.kobi .mouth');
  // Pas encore de texte : bouche au repos, aucune minuterie.
  const still = await mouth.textContent();
  await page.clock.runFor(600);
  expect(await mouth.textContent()).toBe(still);
  // Le texte arrive : la bouche bouge.
  const seen = new Set<string>();
  let text = '';
  for (const word of 'C’est dans api/src/auth/refresh.ts, ligne 88. Durée actuelle : 15 min.'.split(' ')) {
    text = text ? `${text} ${word}` : word;
    await page.evaluate((t) => window.kobi.update({ answer: t }), text);
    for (let i = 0; i < 3; i++) {
      await page.clock.runFor(55);
      seen.add((await mouth.textContent()) ?? '');
    }
  }
  expect(seen.size).toBeGreaterThan(2);
  // Plus rien n'arrive : la bouche se referme.
  await page.clock.runFor(3_000);
  await expect(mouth).toHaveText('·');
  await expectNoErrors(errors);
});

test('le regard et le corps suivent le curseur ; le survol réveille Kobi en veille', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.waitForTimeout(400);
  const gaze = page.locator('.kobi .gaze');
  await page.mouse.move(805, 30);
  await expect.poll(() => gaze.evaluate((n) => n.style.transform)).toMatch(/translate\((1\d|2\d)(\.\d+)?px, -\d+(\.\d+)?px\)/);
  // Réveillé : les yeux s'ouvrent.
  await expect(page.locator('.kobi .g').first()).toHaveText('o');
  expect(await page.locator('.kobi .lean').evaluate((n) => n.style.transform)).toContain('rotate(');
  await page.mouse.move(420, 540);
  await expect.poll(() => gaze.evaluate((n) => n.style.transform)).toMatch(/translate\(-\d+(\.\d+)?px, 1\d(\.\d+)?px\)/);
  await expectNoErrors(errors);
});

test('réduire les animations : le curseur ne fait rien bouger', async ({ page }) => {
  const errors = await openUi(page, 'reduced=1&state=listen');
  await page.mouse.move(805, 30);
  await page.waitForTimeout(200);
  expect(await page.locator('.kobi .lean').evaluate((n) => n.style.transform)).toBe('');
  expect(await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  await expectNoErrors(errors);
});

test('en écoute, Kobi suit le volume du micro, puis s’arrête quand il retombe', async ({ page }) => {
  const errors = await openUi(page, 'state=listen');
  await page.waitForTimeout(700);
  const voice = page.locator('.kobi .voice');
  await page.evaluate(() => window.kobi.setLevel(1));
  await expect.poll(() => voice.evaluate((n) => n.style.transform)).toMatch(/translateY\(-\d/);
  // Sans nouvelle valeur, le volume retombe et la boucle s'arrête.
  await expect.poll(() => voice.evaluate((n) => n.style.transform), { timeout: 4000 }).toBe('');
  // Hors écoute, le volume est ignoré.
  await page.evaluate(() => window.kobi.setState('think', window.kobi.demoData('think')));
  await page.evaluate(() => window.kobi.setLevel(1));
  await page.waitForTimeout(150);
  expect(await voice.evaluate((n) => n.style.transform)).toBe('');
  await expectNoErrors(errors);
});

test('la veille vit : petites animations rares, aléatoires, jamais deux fois la même d’affilée', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  // Chaque geste joué sur le calque .react ou chaque expression des yeux est noté.
  await page.evaluate(() => {
    const log: string[] = [];
    (window as unknown as { life: string[] }).life = log;
    const react = document.querySelector('.kobi .react')!;
    const original = react.animate.bind(react);
    react.animate = (k, o) => (log.push('geste'), original(k, o));
    new MutationObserver(() => log.push('yeux')).observe(document.querySelector('.kobi .g')!, { childList: true });
    new MutationObserver(() => log.push('mot')).observe(document.querySelector('.kobi .word')!, { childList: true, subtree: true, characterData: true });
  });
  await page.clock.runFor(60_000);
  const life = await page.evaluate(() => (window as unknown as { life: string[] }).life);
  expect(life.length).toBeGreaterThan(3);
  expect(new Set(life).size).toBeGreaterThan(1);
  await expectNoErrors(errors);
});

test('pendant une réunion, l’enregistrement se voit dans tous les états, avec la durée et les notes', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, 'backdrop=1');
  const start = await page.evaluate(() => Date.now() - 754_000);
  const rec = (notes: number, pause = false) =>
    page.evaluate(([notes, pause, start]) => window.kobi.recording({ actif: true, pause: Boolean(pause), debut_ms: Number(start), notes: Number(notes), titre: 'Point hebdo' }), [notes, pause, start]);
  await rec(0);
  const ring = page.locator('.kobi .rec-ring');
  const badge = page.locator('.rec');
  await expect(ring).toBeVisible();
  await expect(badge).toContainText('REC · Point hebdo');
  await expect(badge.locator('.rec-time')).toHaveText('12:34');
  await expect(badge.locator('.rec-notes')).toBeHidden();
  // Pas de carte de consentement quand la coque reprend un enregistrement.
  await expect(page.locator('[data-card=consent]')).toHaveCount(0);
  for (const state of STATES) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), state);
    await page.clock.runFor(800);
    await expect(ring).toBeVisible();
    await expect(badge).toBeVisible();
  }
  // Une note ajoutée : le compteur rebondit, un « +1 » s'envole.
  await rec(1);
  await expect(badge.locator('.rec-notes')).toHaveText('1 note');
  await expect(badge.locator('.rec-plus')).toHaveText('+1');
  await rec(3);
  await expect(badge.locator('.rec-notes')).toHaveText('3 notes');
  // La durée défile, sans retour en arrière quand l'état est renvoyé.
  await page.clock.runFor(2_000);
  await expect(badge.locator('.rec-time')).not.toHaveText('12:34');
  // Pause : anneau gris immobile, PAUSE.
  await rec(3, true);
  await expect(page.locator('.kobi')).toHaveClass(/rec-paused/);
  await expect(badge.locator('.rec-label')).toHaveText('PAUSE · Point hebdo');
  await page.evaluate(() => window.kobi.recording({ actif: false, pause: false, debut_ms: 0, notes: 0, titre: null }));
  await expect(ring).toBeHidden();
  await expect(badge).toBeHidden();
  await expectNoErrors(errors);
});

test('Fait ! : une gerbe de symboles jaillit puis disparaît', async ({ page }) => {
  const errors = await openUi(page, 'state=suggest');
  await page.waitForTimeout(600);
  await page.evaluate(() => window.kobi.setState('done', window.kobi.demoData('done')));
  await expect.poll(() => page.locator('.kobi .burst span').count()).toBeGreaterThan(5);
  await expect.poll(() => page.locator('.kobi .burst span').count(), { timeout: 4000 }).toBe(0);
  await expectNoErrors(errors);
});

test('le mot reste lisible sur un fond sombre : contour vert sous le texte noir', async ({ page }) => {
  const errors = await openUi(page, 'backdrop=sombre&state=answer');
  const style = await page.locator('.kobi .word').evaluate((n) => {
    const s = getComputedStyle(n);
    return { stroke: s.webkitTextStroke || `${s.getPropertyValue('-webkit-text-stroke-width')} ${s.getPropertyValue('-webkit-text-stroke-color')}`, order: s.paintOrder, color: s.color };
  });
  expect(style.order).toContain('stroke');
  expect(style.stroke).toMatch(/8px/);
  expect(style.color).toBe('rgb(11, 11, 11)');
  await expectNoErrors(errors);
});

test('pendant l’enregistrement, le repos de Kobi est la prise de notes, puis il y revient après avoir répondu', async ({ page }) => {
  const errors = await openUi(page, 'backdrop=1');
  const kobi = page.locator('.kobi');
  const rec = (r: { notes: number; pause?: boolean; note?: string; actif?: boolean }) =>
    page.evaluate(
      (r) =>
        window.kobi.recording({ actif: r.actif ?? true, pause: Boolean(r.pause), debut_ms: Date.now() - 60_000, notes: r.notes, titre: 'Point hebdo', derniere_note: r.note ?? null }),
      r,
    );
  await expect(kobi).toHaveAttribute('data-state', 'idle');
  // L'enregistrement commence : Kobi se met à prendre des notes, la fiche se tape.
  await rec({ notes: 0 });
  await expect(kobi).toHaveAttribute('data-state', 'write');
  await expect(kobi).toHaveClass(/m-write/);
  await expect(page.locator('[data-card=notes] .text')).toHaveText('J’écoute et je prends des notes.');
  await expect(page.locator('.kobi .rec-ring')).toBeVisible();
  // La dernière note prise, quand la coque l'envoie, est tapée dans la fiche.
  await rec({ notes: 1, note: 'Décision : refresh token à 30 jours' });
  await expect(page.locator('[data-card=notes] .text')).toHaveText('Décision : refresh token à 30 jours');
  await expect(page.locator('.rec-notes')).toHaveText('1 note');
  // Une question : Kobi réfléchit, répond, puis retourne à ses notes.
  for (const s of ['listen', 'think', 'answer'] as const) {
    await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), s);
    await expect(kobi).toHaveAttribute('data-state', s);
  }
  await page.evaluate(() => window.kobi.setState('idle'));
  await expect(kobi).toHaveAttribute('data-state', 'write');
  await expect(page.locator('[data-card=notes] .text')).toHaveText('Décision : refresh token à 30 jours');
  // En pause, plus de transcription : Kobi se repose ; à la reprise, il reprend ses notes.
  await rec({ notes: 1, pause: true });
  await expect(kobi).toHaveAttribute('data-state', 'idle');
  await rec({ notes: 1 });
  await expect(kobi).toHaveAttribute('data-state', 'write');
  // Fin de la réunion : veille.
  await rec({ notes: 1, actif: false });
  await expect(kobi).toHaveAttribute('data-state', 'idle');
  await expect(page.locator('[data-card]')).toHaveCount(0);
  await expectNoErrors(errors);
});

test('la prise de notes passe en mode économe après 20 s : le mot cesse d’onduler', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 0, titre: null }));
  const kobi = page.locator('.kobi');
  await expect(kobi).toHaveClass(/m-write/);
  await page.clock.runFor(21_000);
  await expect(kobi).toHaveClass(/deep/);
  await page.clock.runFor(1_500);
  await page.waitForTimeout(1_400);
  const fontAnimations = await page.evaluate(
    () =>
      document
        .getAnimations()
        .filter((a) => a.playState === 'running')
        .filter((a) => (a.effect as KeyframeEffect).getKeyframes().some((k) => 'fontVariationSettings' in k)).length,
  );
  expect(fontAnimations).toBe(0);
  // Le griffonnage continue.
  expect(await page.locator('.kobi .pill').evaluate((n) => n.getAnimations().length)).toBe(1);
  await expectNoErrors(errors);
});
