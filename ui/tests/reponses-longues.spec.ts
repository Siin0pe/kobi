// Réponses longues (KOB-163) : la bulle grandit jusqu'à un maximum sans tasser
// le texte, défile en douceur en suivant le flux, met en forme listes, code et
// chemins longs, propose « Lire en grand » (vue agrandie : vue.spec.ts).
// Réponses de 50, 300 et 1 500 mots.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

test.use({ video: { mode: 'on', size: { width: 820, height: 560 } } });

// Ces tests suivent la bulle elle-même : mode « bulle détaillée », sans
// ouverture automatique de la vue agrandie (KOB-169, voir vue.spec.ts).
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('kobi.affichage', JSON.stringify({ mode: 'detaille', auto: 'jamais' })));
});

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
  await expect(card(page).getByRole('button', { name: /Lire en grand/ })).toBeHidden();
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

test('à 15 images/s, le suivi rattrape le bas aussi vite qu’à 60 (temps écoulé, pas nombre d’images)', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobiAnimations.poser({ fps: 15 }));
  await answer(page, mots(300));
  await page.clock.runFor(30_000);
  const ecart = () => zone(page).evaluate((n) => n.scrollHeight - n.clientHeight - n.scrollTop);
  await expect.poll(ecart).toBeLessThanOrEqual(6);
  // Un bloc de 300 px arrive d'un coup sous le texte.
  await zone(page).evaluate((n) => {
    const bloc = document.createElement('div');
    bloc.style.height = '300px';
    n.querySelector('.text')!.append(bloc);
  });
  await page.clock.runFor(300);
  // 18 % par 1/60 s : en 300 ms, plus de 95 % de l'écart est rattrapé. Avant
  // KOB-405, 18 % par image : à 15 images/s, 4 ou 5 pas, 40 % restaient.
  expect(await ecart()).toBeLessThan(30);
  await expectNoErrors(errors);
});

test('1 500 mots : début lisible, défilement, et « Lire en grand » qui ouvre la vue agrandie', async ({ page }) => {
  const errors = await openUi(page, '');
  await answer(page, mots(1500));
  await typed(page);
  await lisible(page, 18);
  const all = card(page).getByRole('button', { name: 'Lire en grand ›' });
  await expect(all).toBeVisible();
  // Un bouton ne bouge pas : la bulle ne flotte plus.
  const b1 = await all.boundingBox();
  await page.waitForTimeout(600);
  expect(await all.boundingBox()).toEqual(b1);
  await page.screenshot({ path: test.info().outputPath('1500-mots.png') });
  await all.click();
  await expect(page.getByRole('dialog', { name: 'Conversation avec Kobi' })).toBeVisible();
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

test('KOB-246 : remonter de 2 px depuis le bas arrête le suivi ; un glisser pour défiler n’ouvre pas la vue', async ({ page }) => {
  const errors = await openUi(page, '');
  const full = mots(300);
  await answer(page, full);
  await typed(page);
  await page.waitForTimeout(600);
  // En bas, on remonte à peine (barre, pavé tactile) : le suivi s'arrête quand même.
  const haut = await zone(page).evaluate((n) => {
    n.scrollTop = n.scrollHeight - n.clientHeight - 2;
    return n.scrollTop;
  });
  await page.waitForTimeout(200);
  await page.evaluate((t) => window.kobi.update({ answer: t }), `${full}\n\nUne phrase de plus, puis une autre encore, qui allongent la réponse.`);
  await page.waitForTimeout(800);
  expect(await zone(page).evaluate((n) => n.scrollTop)).toBeLessThanOrEqual(haut + 1);
  // Glisser dans la bulle pour la faire défiler : au relâchement, rien ne s'ouvre.
  const box = (await zone(page).boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 60, { steps: 6 });
  await page.mouse.up();
  await page.waitForTimeout(500);
  await expect(page.getByRole('dialog', { name: 'Conversation avec Kobi' })).toBeHidden();
  await expectNoErrors(errors);
});

test('KOB-246 (4.1) : la bulle qui devient défilante en pleine frappe cesse de flotter sans rejouer son entrée', async ({ page }) => {
  const errors = await openUi(page, '');
  await answer(page, mots(300));
  await expect(zone(page)).toHaveClass(/peut-monter|peut-descendre/, { timeout: 30_000 });
  // Encore en frappe : aucune animation d'entrée relancée, la bulle reste visible.
  const etat = await card(page).evaluate((n) => ({
    noms: n.getAnimations().map((a) => (a as CSSAnimation).animationName),
    lecture: getComputedStyle(n).animationPlayState,
    opacite: Number(getComputedStyle(n).opacity),
  }));
  expect(etat.noms).not.toContain('deal');
  expect(etat.lecture).toBe('paused');
  expect(etat.opacite).toBe(1);
  await expectNoErrors(errors);
});
