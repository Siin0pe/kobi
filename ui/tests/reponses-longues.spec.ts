// Réponses longues (KOB-163) : la bulle grandit jusqu'à un maximum sans tasser
// le texte, défile en douceur en suivant le flux, met en forme listes, code et
// chemins longs, propose « Tout lire dans l'app » ; la fenêtre s'agrandit vers
// le côté libre sans que Kobi bouge. Réponses de 50, 300 et 1 500 mots.
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

test.use({ video: { mode: 'on', size: { width: 820, height: 560 } } });

const MOTS = 'le verrou protège la donnée partagée pendant que chaque fil attend son tour avant de lire ou d’écrire'.split(' ');
function mots(n: number): string {
  const out: string[] = [];
  for (let i = 0; i < n; i++) out.push(MOTS[i % MOTS.length]!);
  // Des phrases et des paragraphes.
  return out
    .join(' ')
    .replace(/((?:\S+ ){14}\S+) /g, '$1. ')
    .replace(/((?:[^.]+\. ){4})/g, '$1\n\n')
    .trim();
}
const CHEMIN = 'C:\\Users\\armag\\Desktop\\un\\dossier\\tres\\profond\\avec\\un\\nom\\de\\fichier\\vraiment\\long\\exemple_de_module_rust.rs';
const RICHE = [
  'Un **mutex** garantit qu’un seul fil touche la donnée à la fois :',
  '',
  '- on prend le verrou avec `lock()` ;',
  '- on lit ou on modifie la donnée ;',
  '- le verrou se libère à la fin du bloc.',
  '',
  '```rust',
  'let compteur = Arc::new(Mutex::new(0));',
  'let mut n = compteur.lock().unwrap(); *n += 1; // une ligne de code bien plus large que la bulle, qui défile à part',
  '```',
  '',
  `Le module est dans ${CHEMIN}, et la doc sur https://doc.rust-lang.org/std/sync/struct.Mutex.html#examples-et-encore-plus-loin.`,
].join('\n');

const card = (page: Page) => page.locator('[data-card=answer]');
const zone = (page: Page) => page.locator('[data-card=answer] .defile');

async function answer(page: Page, text: string): Promise<void> {
  await page.evaluate((t) => window.kobi.setState('answer', { answer: t }), text);
}

/** Attend la fin de la frappe (1 500 mots se tapent en temps réel : ~15 s au calme, davantage sur un PC chargé, KOB-172). */
async function typed(page: Page): Promise<void> {
  await expect(card(page)).toHaveAttribute('data-typing', '0', { timeout: 45_000 });
}

/** Rien ne sort de la fenêtre, le texte garde sa taille. */
async function lisible(page: Page, fontPx: number): Promise<void> {
  const box = (await card(page).boundingBox())!;
  const vp = page.viewportSize()!;
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.y + box.height).toBeLessThanOrEqual(vp.height);
  expect(box.x + box.width).toBeLessThanOrEqual(vp.width);
  // Taille réelle du texte à l'écran : jamais tassé.
  const real = await card(page).evaluate((n) => {
    const p = n.querySelector('.md p, .md li') ?? n.querySelector('.md')!;
    const span = p.querySelector('.w') ?? p;
    return span.getBoundingClientRect().height;
  });
  expect(real).toBeGreaterThanOrEqual(fontPx * 0.95);
  // Aucun mot plus large que la bulle (chemins coupés) ; le code défile à part.
  const trop = await card(page).evaluate((n) => {
    const r = n.getBoundingClientRect();
    return [...n.querySelectorAll('.w')].filter((w) => w.getBoundingClientRect().right > r.right + 1).length;
  });
  expect(trop).toBe(0);
}

test('50 mots : la bulle grandit avec la réponse, sans défilement ni dégradé', async ({ page }) => {
  const errors = await openUi(page, '');
  await answer(page, mots(50));
  await typed(page);
  await lisible(page, 18);
  const z = await zone(page).evaluate((n) => ({ s: n.scrollHeight, c: n.clientHeight }));
  expect(z.s).toBeLessThanOrEqual(z.c + 2);
  await expect(zone(page)).not.toHaveClass(/peut-descendre|peut-monter/);
  await expect(card(page).getByRole('button', { name: /Tout lire/ })).toBeHidden();
  await page.screenshot({ path: test.info().outputPath('50-mots.png') });
  await expectNoErrors(errors);
});

test('300 mots : hauteur maximale, puis défilement doux qui suit le flux ; on remonte, il s’arrête ; en bas, il reprend', async ({ page }) => {
  const errors = await openUi(page, '');
  const full = mots(300);
  // En flux, morceau par morceau, comme la coque.
  await page.evaluate(() => window.kobi.setState('answer', { answer: '' }));
  for (let i = 1; i <= 10; i++) {
    await page.evaluate((t) => window.kobi.update({ answer: t }), full.slice(0, Math.round((full.length * i) / 10)));
    await page.waitForTimeout(250);
  }
  await typed(page);
  await page.waitForTimeout(600);
  await lisible(page, 18);
  // La bulle a atteint sa hauteur maximale : le texte défile, il ne déborde pas.
  const z = await zone(page).evaluate((n) => ({ s: n.scrollHeight, c: n.clientHeight, t: n.scrollTop }));
  expect(z.s).toBeGreaterThan(z.c + 50);
  // Le suivi l'a amenée en bas.
  expect(z.t + z.c).toBeGreaterThanOrEqual(z.s - 6);
  await expect(zone(page)).toHaveClass(/peut-monter/);
  await page.screenshot({ path: test.info().outputPath('300-mots-bas.png') });
  // On remonte à la molette : le suivi s'arrête, l'historique ne s'ouvre pas.
  const box = (await zone(page).boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, -400);
  await page.waitForTimeout(500);
  await expect(page.getByRole('log', { name: 'Échanges récents avec Kobi' })).toBeHidden();
  const up = await zone(page).evaluate((n) => n.scrollTop);
  expect(up).toBeLessThan(z.t - 100);
  await expect(zone(page)).toHaveClass(/peut-descendre/);
  // La suite du flux n'arrache pas la lecture.
  await page.evaluate((t) => window.kobi.update({ answer: t }), `${full}\n\nEt une dernière phrase qui arrive après coup.`);
  await page.waitForTimeout(700);
  expect(await zone(page).evaluate((n) => n.scrollTop)).toBeLessThanOrEqual(up + 2);
  await page.screenshot({ path: test.info().outputPath('300-mots-remonte.png') });
  // Glisser vers le haut (doigt ou souris) redescend ; revenu en bas, le suivi reprend.
  await page.mouse.wheel(0, 4000);
  await page.waitForTimeout(500);
  await page.evaluate((t) => window.kobi.update({ answer: t }), `${full}\n\nEt une dernière phrase qui arrive après coup. Puis encore une autre, pour voir si on suit bien le flux jusqu’au bout.`);
  // Pendant la frappe, la bulle suit…
  await page.waitForTimeout(1_500);
  expect(await zone(page).evaluate((n) => n.scrollTop + n.clientHeight - n.scrollHeight)).toBeGreaterThanOrEqual(-40);
  await typed(page);
  await page.waitForTimeout(600);
  const end = await zone(page).evaluate((n) => n.scrollTop + n.clientHeight - n.scrollHeight);
  expect(end).toBeGreaterThanOrEqual(-6);
  await expectNoErrors(errors);
});

test('1 500 mots : début lisible, défilement, et « Tout lire dans l’app » qui ouvre l’app', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await answer(page, mots(1500));
  await typed(page);
  await lisible(page, 18);
  const all = card(page).getByRole('button', { name: 'Tout lire dans l’app ›' });
  await expect(all).toBeVisible();
  // Un bouton ne bouge pas : la bulle ne flotte plus.
  const b1 = await all.boundingBox();
  await page.waitForTimeout(600);
  expect(await all.boundingBox()).toEqual(b1);
  await page.screenshot({ path: test.info().outputPath('1500-mots.png') });
  await all.click();
  expect(await calls(page)).toContainEqual(['ouvrir_app', { section: 'notes' }]);
  await expectNoErrors(errors);
});

test('mise en forme : paragraphes, liste, code en bloc qui défile à part, chemins et adresses coupés', async ({ page }) => {
  const errors = await openUi(page, '');
  await answer(page, RICHE);
  await typed(page);
  const md = card(page).locator('.md');
  await expect(md.locator('ul > li')).toHaveCount(3);
  await expect(md.locator('ul > li').first()).toContainText('on prend le verrou avec lock()');
  await expect(md.locator('.md-code').first()).toHaveText('lock()');
  await expect(md.locator('strong')).toHaveText('mutex');
  const pre = md.locator('pre code');
  await expect(pre).toContainText('let compteur = Arc::new(Mutex::new(0));');
  expect(await pre.textContent()).not.toContain('```');
  expect(await pre.textContent()).not.toMatch(/^rust/);
  // Le code garde ses lignes et défile horizontalement dans son bloc.
  const code = await md.locator('pre').evaluate((n) => ({ s: n.scrollWidth, c: n.clientWidth, ws: getComputedStyle(n).whiteSpace }));
  expect(code.ws).toBe('pre');
  expect(code.s).toBeGreaterThan(code.c);
  await lisible(page, 18);
  await zone(page).evaluate((n) => (n.scrollTop = n.scrollHeight));
  await page.waitForTimeout(300);
  await page.screenshot({ path: test.info().outputPath('mise-en-forme.png') });
  await expectNoErrors(errors);
});

test('fenêtre agrandie vers le côté libre : Kobi ne bouge pas, la réponse se lit à sa vraie taille, puis tout reprend sa place', async ({ page }) => {
  await page.setViewportSize({ width: 410, height: 280 });
  // La coque simulée agrandit la fenêtre (ici, la vue) quand l'interface le demande.
  await page.exposeFunction('__resize', (w: number, h: number) => page.setViewportSize({ width: w, height: h }));
  await mockTauri(page);
  await page.addInitScript(() => {
    const w = window as unknown as { __TAURI__: { core: { invoke: (c: string, a?: Record<string, unknown>) => Promise<unknown> } }; __resize(w: number, h: number): Promise<void> };
    const base = w.__TAURI__.core.invoke;
    w.__TAURI__.core.invoke = async (cmd, args) => {
      const out = await base(cmd, args);
      if (cmd !== 'agrandir_kobi') return out;
      if (args?.appliquer) await w.__resize(args.grand ? 830 : 410, args.grand ? 620 : 280);
      return args?.grand ? { gauche: true, haut: true } : null;
    };
  });
  const errors = await openUi(page, '');
  const pill = () => page.locator('.kobi .pill').boundingBox();
  const fromCorner = async () => {
    // Boucles de Kobi (respiration…) ramenées à leur première image : la mesure
    // ne dépend plus de la phase où elle tombe (KOB-172).
    await page.evaluate(() => {
      for (const a of document.querySelector('.kobi')!.getAnimations({ subtree: true })) {
        if (a.effect?.getTiming().iterations === Infinity) a.currentTime = 0;
      }
    });
    const b = (await pill())!;
    const vp = page.viewportSize()!;
    return { right: Math.round(vp.width - (b.x + b.width)), bottom: Math.round(vp.height - (b.y + b.height)), w: Math.round(b.width) };
  };
  await page.waitForTimeout(400);
  const before = await fromCorner();
  await page.screenshot({ path: test.info().outputPath('agrandie-avant.png') });
  // Une tâche longue vient d'être livrée : sa notification ne reste pas par-dessus la réponse (sobriété).
  await emit(page, 'conversation', [{ tour: 1, question: 'explique le mutex', reponse: '', statut: 'tache', clavier: true }]);
  await emit(page, 'conversation', [{ tour: 1, question: 'explique le mutex', reponse: RICHE, statut: 'fini', clavier: true }]);
  await expect(page.locator('.notif')).toBeVisible();
  await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'answer', RICHE + '\n\n' + mots(300));
  await expect(page.locator('.stage')).toHaveClass(/agrandie/, { timeout: 10_000 });
  await expect.poll(() => page.viewportSize()!.width, { timeout: 10_000 }).toBe(830);
  await typed(page);
  await page.waitForTimeout(800);
  // Kobi : même taille, même place par rapport au coin bas droit (la fenêtre a grandi vers la gauche et le haut).
  const after = await fromCorner();
  expect(Math.abs(after.right - before.right)).toBeLessThanOrEqual(2);
  expect(Math.abs(after.bottom - before.bottom)).toBeLessThanOrEqual(2);
  expect(Math.abs(after.w - before.w)).toBeLessThanOrEqual(2);
  // La réponse : texte à 16 px réels (et non 9 px tassés), dans la fenêtre, sans chevaucher Kobi.
  await lisible(page, 16);
  const c = (await card(page).boundingBox())!;
  const k = (await pill())!;
  expect(c.x + c.width).toBeLessThanOrEqual(k.x + 1);
  await expect(page.locator('.notifs')).toBeHidden();
  await page.screenshot({ path: test.info().outputPath('agrandie-reponse.png') });
  expect(await calls(page)).toContainEqual(['agrandir_kobi', { grand: true, appliquer: true }]);
  // Fin de la réponse : la fenêtre reprend sa taille, Kobi n'a pas bougé.
  await emit(page, 'state', { etat: 'assistant', activite: null, mascotte: 'idle', transcrit: false });
  await expect.poll(() => page.viewportSize()!.width, { timeout: 10_000 }).toBe(410);
  await expect(page.locator('.stage')).not.toHaveClass(/agrandie/);
  await page.waitForTimeout(300);
  const back = await fromCorner();
  for (const k of ['right', 'bottom', 'w'] as const) expect(Math.abs(back[k] - before[k]), k).toBeLessThanOrEqual(2);
  await page.screenshot({ path: test.info().outputPath('agrandie-apres.png') });
  await expectNoErrors(errors);
});

test('réduire les animations : pas de glissement ni de dégradé animé, le texte est posé et lisible', async ({ page }) => {
  const errors = await openUi(page, 'reduced=1');
  await answer(page, mots(300));
  await typed(page);
  await lisible(page, 18);
  const z = await zone(page).evaluate((n) => ({ s: n.scrollHeight, c: n.clientHeight, t: n.scrollTop, tr: getComputedStyle(n).transitionDuration }));
  expect(z.t + z.c).toBeGreaterThanOrEqual(z.s - 6);
  expect(z.tr.split(',').every((d) => parseFloat(d) === 0)).toBe(true);
  const live = await card(page).evaluate((n) => n.getAnimations({ subtree: true }).filter((a) => a.playState === 'running').length);
  expect(live).toBe(0);
  await expectNoErrors(errors);
});
