// Étapes repliées sous chaque réponse de la vue agrandie (KOB-213) : un clic
// les déplie toujours, même quand la main bouge un peu pendant le clic, que la
// réponse arrive en flux, que Kobi fasse un geste ou qu'on ait défilé. Dans
// l'app, un appui qui bouge de quelques pixels lançait le déplacement de la
// fenêtre (`startDragging`) : la coque prend alors la souris et le clic n'a
// jamais lieu. La coque simulée fait de même.
import { expect, test, type Locator, type Page } from '@playwright/test';
import { emit, expectNoErrors, mockTauri, openUi } from './helpers';

const vue = (page: Page) => page.getByRole('dialog', { name: 'Conversation avec Kobi' });

const etape = (tour: number, id: string, resume: string) => ({ tour, id, outil: 'Grep', type: 'recherche', resume, chemin_relatif: null, classe: 'lecture', at_ms: 0 });
const echange = (tour: number, n = 1, statut = 'fini') => ({
  tour,
  question: `question ${tour}`,
  reponse: `Réponse ${tour}. `.repeat(12),
  statut,
  clavier: true,
  etapes: Array.from({ length: n }, (_, i) => etape(tour, `t${tour}-${i}`, `cherche « mot ${i} »`)),
});

/** Kobi avec la vue agrandie ouverte dans une grande fenêtre ; la coque simulée compte les déplacements de fenêtre. */
async function ouvrir(page: Page): Promise<string[]> {
  await page.setViewportSize({ width: 900, height: 620 });
  await mockTauri(page);
  await page.addInitScript(() => {
    localStorage.setItem('kobi.affichage', JSON.stringify({ v: 2, auto: 'jamais', delai: 0 }));
    const w = window as unknown as { __TAURI__: { window: unknown }; __deplacements: number };
    w.__deplacements = 0;
    const sonde: unknown[] = [];
    (w as unknown as { __sonde: unknown[] }).__sonde = sonde;
    const desc = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop')!;
    Object.defineProperty(Element.prototype, 'scrollTop', {
      get() { return desc.get!.call(this); },
      set(v: number) {
        if ((this as Element).classList?.contains('lecture-corps')) sonde.push({ t: Math.round(performance.now()), ecrit: Math.round(v), avant: Math.round(desc.get!.call(this)) });
        desc.set!.call(this, v);
      },
      configurable: true,
    });
    for (const type of ['pointerdown', 'pointerup', 'click', 'toggle']) {
      window.addEventListener(type, (e) => {
        const cible = e.target as Element;
        const corps = document.querySelector('.lecture-corps');
        const box = document.querySelectorAll('.vex-etapes')[3]?.querySelector('summary')?.getBoundingClientRect();
        sonde.push({ t: Math.round(performance.now()), type, cible: `${cible.tagName}.${cible.className}`, x: (e as MouseEvent).clientX, y: (e as MouseEvent).clientY, st: corps && Math.round(desc.get!.call(corps)), sh: corps?.scrollHeight, ch: corps?.clientHeight, box3: box && [Math.round(box.top), Math.round(box.bottom)], depl: w.__deplacements, dp: e.defaultPrevented });
      }, true);
    }
    let pris = false;
    // Comme la vraie fenêtre : une fois le déplacement lancé, la coque a la
    // souris et le relâchement n'arrive pas à la page, donc pas de clic.
    window.addEventListener('click', (e) => {
      if (!pris) return;
      pris = false;
      e.preventDefault();
      e.stopPropagation();
    }, true);
    w.__TAURI__.window = { getCurrentWindow: () => ({ startDragging: async () => { w.__deplacements++; pris = true; } }) };
    // La coque pose la vue dans la fenêtre (panneau à gauche, Kobi en bas à droite).
    const t = w.__TAURI__ as unknown as { core: { invoke: (c: string, a?: Record<string, unknown>) => Promise<unknown> } };
    const base = t.core.invoke;
    t.core.invoke = async (cmd, args) =>
      cmd === 'agrandir_kobi' && args?.grand
        ? { kobi: { x: 490, y: 340, largeur: 410, hauteur: 280 }, panneau: { x: 16, y: 16, largeur: 520, hauteur: 588 } }
        : base(cmd, args);
  });
  const errors = await openUi(page, '');
  await emit(page, 'conversation', [echange(1, 2), echange(2, 1), echange(3, 3)]);
  await emit(page, 'vue', { action: 'ouvrir' });
  await expect(vue(page).locator('.vex-etapes')).toHaveCount(3);
  // La vue a fini d'entrer : les mesures de position restent justes.
  await vue(page).evaluate((v) => Promise.all(v.getAnimations({ subtree: true }).filter((a) => a.effect?.getTiming().iterations !== Infinity).map((a) => a.finished)));
  return errors;
}

/** Clic de la main : appui, petit glissement de 6 px, relâchement. */
async function cliquer(page: Page, cible: Locator): Promise<void> {
  await cible.scrollIntoViewIfNeeded();
  // On vise une ligne posée, comme la main et comme `locator.click()` : après
  // une mise à jour de la conversation, le suivi glisse encore vers le bas, et
  // un appui mesuré avant tombait à côté sur un PC lent (Windows, KOB-376).
  // Le suivi retenu pendant l'appui, lui, reste vérifié par le clic.
  await cible.evaluate(
    (n) =>
      new Promise<void>((ok) => {
        let avant = Number.NaN;
        let stables = 0;
        const pas = (): void => {
          const y = n.getBoundingClientRect().top;
          stables = y === avant ? stables + 1 : 0;
          avant = y;
          if (stables >= 3) ok();
          else requestAnimationFrame(pas);
        };
        requestAnimationFrame(pas);
      }),
  );
  const b = (await cible.boundingBox())!;
  const x = b.x + Math.min(40, b.width / 2);
  const y = b.y + b.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + 4, y + 3, { steps: 3 });
  await page.mouse.up();
}

async function deplier(page: Page, box: Locator): Promise<void> {
  await expect(box).not.toHaveAttribute('open');
  await page.evaluate(() => ((window as unknown as { __sonde: unknown[] }).__sonde.length = 0));
  const avant = await box.locator('summary').boundingBox();
  await cliquer(page, box.locator('summary'));
  try {
    await expect(box).toHaveAttribute('open', '', { timeout: 3000 });
  } catch (e) {
    const sonde = await page.evaluate(() => (window as unknown as { __sonde: unknown[] }).__sonde);
    throw new Error(`SONDE summary avant ${JSON.stringify(avant)}\n${sonde.map((x) => JSON.stringify(x)).join('\n')}\n${e}`);
  }
  // Replié de nouveau pour le clic suivant (clic simple, sans glisser).
  await box.locator('summary').click();
  await expect(box).not.toHaveAttribute('open');
}

test('50 clics sur « N étapes », dans toutes les situations : 50 dépliages, la fenêtre ne bouge jamais', async ({ page }) => {
  const errors = await ouvrir(page);
  const boxes = vue(page).locator('.vex-etapes');
  let clics = 0;
  // 1. Après la réponse : dernière et anciennes réponses.
  for (let i = 0; i < 12; i++, clics++) await deplier(page, boxes.nth(i % 3));
  // 2. Pendant une réponse en flux (nouveau tour qui s'écrit sous les autres).
  await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'question', 'question 4');
  for (let i = 0; i < 10; i++, clics++) {
    await emit(page, 'answer', 'Réponse 4 en cours. '.repeat(i + 1));
    await deplier(page, boxes.nth(i % 3));
  }
  // 3. Pendant un geste de Kobi (états qui changent) et une mise à jour d'activité.
  for (let i = 0; i < 10; i++, clics++) {
    await emit(page, 'state', { etat: 'assistant', activite: 'travaille', mascotte: i % 2 ? 'work' : 'think', transcrit: false });
    await emit(page, 'conversation', [echange(1, 2), echange(2, 1), echange(3, 3), echange(4, 1 + (i % 3), 'en_cours')]);
    await deplier(page, boxes.nth(i % 4));
  }
  // 4. Après défilement, tout en haut puis tout en bas.
  const corps = vue(page).locator('.lecture-corps');
  for (let i = 0; i < 18; i++, clics++) {
    await corps.evaluate((c, haut) => (c.scrollTop = haut ? 0 : c.scrollHeight), i % 2 === 0);
    await deplier(page, boxes.nth(i % 2 === 0 ? 0 : 3));
  }
  expect(clics).toBe(50);
  expect(await page.evaluate(() => (window as unknown as { __deplacements: number }).__deplacements)).toBe(0);
  await expectNoErrors(errors);
});

test('clic pendant que la réponse en flux s’allonge en bas de la vue : la ligne ne monte pas sous le doigt, la boîte s’ouvre (KOB-376)', async ({ page }) => {
  const errors = await ouvrir(page);
  const box = vue(page).locator('.vex-etapes').nth(2);
  const corps = vue(page).locator('.lecture-corps');
  await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'question', 'question 4');
  await emit(page, 'answer', 'Réponse 4 en cours. ');
  await corps.evaluate((c) => (c.scrollTop = c.scrollHeight));
  const enBas = () => corps.evaluate((c) => c.scrollHeight - c.clientHeight - c.scrollTop);
  await expect.poll(enBas).toBeLessThan(2);
  const s = box.locator('summary');
  const b = (await s.boundingBox())!;
  await page.mouse.move(b.x + 40, b.y + b.height / 2);
  await page.mouse.down();
  // Le texte s'allonge pendant l'appui, tenu comme une main qui hésite : le
  // suivi ne glisse pas sous le doigt.
  await emit(page, 'answer', 'Réponse 4 en cours. '.repeat(30));
  await page.waitForTimeout(300);
  expect(Math.abs((await s.boundingBox())!.y - b.y)).toBeLessThan(1);
  await page.mouse.up();
  await expect(box).toHaveAttribute('open', '');
  // Relâché : le suivi reprend jusqu'en bas.
  await expect.poll(enBas).toBeLessThan(2);
  await expectNoErrors(errors);
});

test('l’état déplié est gardé par réponse quand l’historique se redessine (nouvelles étapes, nouvelle réponse)', async ({ page }) => {
  const errors = await ouvrir(page);
  const boxes = vue(page).locator('.vex-etapes');
  await boxes.nth(0).locator('summary').click();
  await boxes.nth(2).locator('summary').click();
  // Une étape de plus sur la réponse 3, puis une réponse 4 : rien ne se replie.
  await emit(page, 'conversation', [echange(1, 2), echange(2, 1), echange(3, 4), echange(4, 1)]);
  await expect(boxes).toHaveCount(4);
  await expect(boxes.nth(2).locator('summary')).toHaveText('4 étapes');
  await expect(boxes.nth(0)).toHaveAttribute('open', '');
  await expect(boxes.nth(1)).not.toHaveAttribute('open');
  await expect(boxes.nth(2)).toHaveAttribute('open', '');
  await expect(boxes.nth(2).locator('.vex-etapes-liste > *')).toHaveCount(4);
  await expect(boxes.nth(3)).not.toHaveAttribute('open');
  await expectNoErrors(errors);
});

test('clavier : Tab jusqu’à « N étapes », Entrée et Espace déplient et replient', async ({ page }) => {
  const errors = await ouvrir(page);
  const box = vue(page).locator('.vex-etapes').nth(1);
  await box.locator('summary').focus();
  await page.keyboard.press('Enter');
  await expect(box).toHaveAttribute('open', '');
  await page.keyboard.press('Space');
  await expect(box).not.toHaveAttribute('open');
  await expectNoErrors(errors);
});

test('la ligne entière est cliquable, curseur main et retour au survol', async ({ page }) => {
  const errors = await ouvrir(page);
  const box = vue(page).locator('.vex-etapes').nth(0);
  const s = box.locator('summary');
  const ligne = (await vue(page).locator('.vex').nth(0).boundingBox())!;
  const b = (await s.boundingBox())!;
  // Toute la largeur de la réponse, pas seulement le texte « 2 étapes ».
  expect(b.width).toBeGreaterThan(ligne.width - 4);
  expect(await s.evaluate((n) => getComputedStyle(n).cursor)).toBe('pointer');
  const avant = await s.evaluate((n) => getComputedStyle(n).backgroundColor);
  await s.hover();
  await expect.poll(() => s.evaluate((n) => getComputedStyle(n).backgroundColor)).not.toBe(avant);
  // Un clic tout à droite de la ligne déplie aussi.
  const d = (await s.boundingBox())!;
  await page.mouse.click(d.x + d.width - 6, d.y + d.height / 2);
  await expect(box).toHaveAttribute('open', '');
  await expectNoErrors(errors);
});
