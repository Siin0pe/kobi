// Écrire à Kobi (KOB-147) : plus de barre permanente ; une bulle qui sort de
// Kobi (clic, touche, menu, raccourci), le texte aspiré à l'envoi, rangée par
// Échap ou après 8 s vide. Chaque test est filmé (preuve du ticket).
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, kobiPose, mockTauri, openUi } from './helpers';

test.use({ video: { mode: 'on', size: { width: 820, height: 560 } } });

const bulle = (page: Page) => page.locator('form.ecrire');
const champ = (page: Page) => page.getByRole('textbox', { name: 'Écrire à Kobi' });

/** Pose la souris loin de Kobi : le regard ne suit que la bulle. */
async function away(page: Page): Promise<void> {
  await page.mouse.move(5, 5);
}

/** La bulle a fini de grandir. */
async function opened(page: Page): Promise<void> {
  await expect(bulle(page)).toHaveAttribute('data-etat', 'ouverte');
  await expect(champ(page)).toBeFocused();
  await expect.poll(() => bulle(page).evaluate((n) => n.getAnimations().length)).toBe(0);
}

test('au repos, rien d’autre que Kobi : aucune barre, aucun champ visible', async ({ page }) => {
  const errors = await openUi(page, '');
  await kobiPose(page);
  await expect(bulle(page)).toBeHidden();
  await expect(champ(page)).toBeHidden();
  await expect(page.locator('.say')).toHaveCount(0);
  await page.screenshot({ path: test.info().outputPath('repos.png') });
  await expectNoErrors(errors);
});

test('clic sur Kobi : la bulle grandit depuis sa bouche avec un rebond, pointe vers lui', async ({ page }) => {
  const errors = await openUi(page, '');
  await kobiPose(page);
  const mouth = await page.locator('.kobi .mouth').boundingBox();
  await page.mouse.click(580, 250);
  await expect(bulle(page)).toBeVisible();
  // En vol : elle part petite, près de la bouche…
  const start = await bulle(page).evaluate((n) => {
    const a = n.getAnimations()[0]!;
    a.pause();
    a.currentTime = 0;
    const r = n.getBoundingClientRect();
    return { w: r.width, right: r.right, top: r.top };
  });
  expect(start.w).toBeLessThan(80);
  expect(Math.abs(start.right - (mouth!.x + mouth!.width / 2))).toBeLessThan(60);
  // … dépasse sa taille (rebond)…
  const over = await bulle(page).evaluate((n) => {
    const a = n.getAnimations()[0]!;
    a.currentTime = 290;
    const w = n.getBoundingClientRect().width;
    a.play();
    return w;
  });
  expect(over).toBeGreaterThan(360);
  await opened(page);
  expect((await bulle(page).boundingBox())!.width).toBeCloseTo(360, 0);
  // La pointe monte vers Kobi, et la bulle est sous les bulles de réponse.
  const box = (await bulle(page).boundingBox())!;
  expect(box.x + box.width).toBeLessThan(mouth!.x);
  expect(box.y).toBeGreaterThan(mouth!.y);
  await page.screenshot({ path: test.info().outputPath('ouverte.png') });
  await expectNoErrors(errors);
});

test('n’importe quelle touche ouvre la bulle et la lettre y arrive', async ({ page }) => {
  const errors = await openUi(page, '');
  await away(page);
  await page.keyboard.type('bonjour');
  await expect(champ(page)).toHaveValue('bonjour');
  await expect(bulle(page)).toHaveAttribute('data-etat', 'ouverte');
  await expectNoErrors(errors);
});

test('la touche qui ouvre la bulle est écrite dedans, une seule fois (KOB-163)', async ({ page }) => {
  const errors = await openUi(page, '');
  await away(page);
  // Une vraie frappe : la première lettre n'est ni avalée ni doublée.
  await page.keyboard.press('k');
  await expect(champ(page)).toHaveValue('k');
  await page.keyboard.type('obi');
  await expect(champ(page)).toHaveValue('kobi');
  await expect(page.locator('.ecrire-miroir')).toHaveText('kobi​');
  await page.keyboard.press('Escape');
  await expect(bulle(page)).toBeHidden();
  // Cas de WebView2 : la touche n'est pas redirigée vers le champ qui vient
  // d'apparaître (événement non natif ici) ; la bulle l'écrit quand même.
  await page.evaluate(() => document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'é', bubbles: true, cancelable: true })));
  await expect(champ(page)).toHaveValue('é');
  await expect(champ(page)).toBeFocused();
  await page.keyboard.type('crire');
  await expect(champ(page)).toHaveValue('écrire');
  await expectNoErrors(errors);
});

test('frappe rapide sans ouvrir la bulle : aucune des premières touches n’est perdue (KOB-163)', async ({ page }) => {
  const errors = await openUi(page, '');
  await away(page);
  // Vraie frappe, sans délai entre les touches.
  await page.keyboard.type('Dans le dossier', { delay: 0 });
  await expect(champ(page)).toHaveValue('Dans le dossier');
  await page.keyboard.press('Escape');
  await expect(bulle(page)).toBeHidden();
  // Cas de WebView2 : le focus du champ n'est pas encore pris, les trois
  // premières touches arrivent encore sur la page, d'un seul coup.
  await page.evaluate(() => {
    for (const key of ['D', 'a', 'n']) document.body.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  });
  await page.keyboard.type('s le dossier', { delay: 0 });
  await expect(champ(page)).toHaveValue('Dans le dossier');
  await expect(page.locator('.ecrire-miroir')).toHaveText('Dans le dossier​');
  await expectNoErrors(errors);
});

test('entrée « Écrire à Kobi » du menu (événement kobi:ecrire) et raccourci global (événement ecrire de la coque)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('kobi:ecrire')));
  await opened(page);
  await page.keyboard.press('Escape');
  await expect(bulle(page)).toBeHidden();
  await emit(page, 'ecrire', null);
  await opened(page);
  await page.keyboard.press('Escape');
  await expect(bulle(page)).toBeHidden();
  // Entrée `ecrire` du menu du clic droit (événement kobi:menu, KOB-139).
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('kobi:menu', { detail: 'ecrire' })));
  await opened(page);
  await expectNoErrors(errors);
});

test('pendant la frappe : lettres brouillées puis posées, Kobi suit le curseur des yeux et sursaute', async ({ page }) => {
  // Sursauts de la pilule comptés au moment où ils sont joués : trop brefs
  // pour être lus à coup sûr sous charge (KOB-172).
  await page.addInitScript(() => {
    const w = window as unknown as { __sursauts: number };
    w.__sursauts = 0;
    const animate = Element.prototype.animate;
    Element.prototype.animate = function (this: Element, ...args: Parameters<Element['animate']>) {
      const options = args[1];
      if (this.classList.contains('pill') && typeof options === 'object' && options?.composite === 'add') w.__sursauts++;
      return animate.apply(this, args);
    };
  });
  const errors = await openUi(page, '');
  await away(page);
  await page.evaluate(() => window.kobi.ecrire());
  await opened(page);
  // Yeux relevés pendant chaque impatience (voir la fin du test) : au moment
  // même, puis à chaque fois qu'ils se repeignent jusqu'à sa fin. Sous
  // charge, après un temps mort, l'impatience tombe dans le clignement dû en
  // même temps (sa réouverture n'est programmée qu'à sa fermeture) : le
  // regard en coin n'arrive que 120 ms plus tard, un seul relevé au moment
  // même lisait « – » (KOB-421).
  await page.evaluate(() => {
    const w = window as unknown as { __impatience: string[] };
    w.__impatience = [];
    const kobi = window.kobi.stage.kobi;
    const impatient = kobi.impatient.bind(kobi);
    kobi.impatient = () => {
      impatient();
      const oeil = document.querySelector('.kobi .g')!;
      const relever = (): void => void w.__impatience.push(oeil.textContent ?? '');
      relever();
      const suivi = new MutationObserver(relever);
      suivi.observe(oeil, { childList: true, characterData: true, subtree: true });
      setTimeout(() => suivi.disconnect(), 900);
    };
  });
  const gaze = () => page.locator('.kobi .gaze').evaluate((n) => new DOMMatrix(getComputedStyle(n).transform).m41);
  // Kobi est réveillé et regarde vers la bulle (à sa gauche).
  await expect(page.locator('.kobi .g').first()).toHaveText('o');
  await expect.poll(gaze).toBeLessThan(-10);
  await page.keyboard.type('où est', { delay: 90 });
  // Une lettre qui vient d'arriver est d'abord un symbole de Kobi.
  await page.keyboard.press('x');
  const just = page.locator('.ecrire-miroir .ecrire-c').last();
  await expect(just).toHaveClass(/nouv/);
  expect(await just.textContent()).not.toBe('x');
  await expect(just).toHaveText('x');
  await expect(page.locator('.ecrire-miroir')).toHaveText('où estx​');
  // Chaque touche fait sursauter la pilule.
  const avant = await page.evaluate(() => (window as unknown as { __sursauts: number }).__sursauts);
  await page.keyboard.press('y');
  await expect.poll(() => page.evaluate(() => (window as unknown as { __sursauts: number }).__sursauts)).toBeGreaterThan(avant);
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  // Le regard suit le curseur : au début du texte, il est plus à gauche qu'à la fin.
  await page.keyboard.type(' géré le refresh token ?', { delay: 60 });
  // Regard posé en fin de texte (deux relevés égaux), puis il repart vers le début.
  let atEnd = NaN;
  await expect
    .poll(async () => {
      const g = await gaze();
      const pose = g === atEnd;
      atEnd = g;
      return pose;
    })
    .toBe(true);
  await page.keyboard.press('Home');
  await expect.poll(gaze).toBeLessThan(atEnd);
  await page.keyboard.press('End');
  // Le miroir est exactement sous le champ : les lettres dessinées sont celles tapées.
  await expect(page.locator('.ecrire-miroir')).toHaveText('où est géré le refresh token ?​');
  // On hésite : Kobi s'impatiente (regard en coin). Le regard ne dure que
  // 900 ms : relevé au moment de l'impatience plutôt qu'échantillonné (KOB-172).
  await expect.poll(() => page.evaluate(() => (window as unknown as { __impatience: string[] }).__impatience), { timeout: 8_000 }).toContain('¬');
  await page.screenshot({ path: test.info().outputPath('frappe.png') });
  await expectNoErrors(errors);
});

test('copier-coller marche dans la bulle ; le texte s’y sélectionne, pas le reste de Kobi', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.ecrire());
  await opened(page);
  await page.keyboard.type('refresh token');
  await page.keyboard.press('Control+A');
  await page.keyboard.press('Control+C');
  await page.keyboard.press('End');
  await page.keyboard.type(' et ');
  await page.keyboard.press('Control+V');
  await expect(champ(page)).toHaveValue('refresh token et refresh token');
  await expect(page.locator('.ecrire-miroir')).toHaveText('refresh token et refresh token​');
  expect(await champ(page).evaluate((n) => getComputedStyle(n).userSelect)).toBe('text');
  await expectNoErrors(errors);
});

test('Entrée : le texte est aspiré dans Kobi qui l’avale, puis réfléchit ; envoyer_texte part', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.ecrire());
  await opened(page);
  await page.keyboard.type('où est géré le refresh token ?');
  await expect(page.locator('.ecrire-c.nouv')).toHaveCount(0);
  await page.keyboard.press('Enter');
  // Les lettres volent vers la bouche ; la bulle se vide tout de suite.
  await expect(page.locator('.ecrire-vole').first()).toBeAttached();
  await expect(champ(page)).toHaveValue('');
  await expect(page.locator('.kobi .mouth')).toHaveText('O');
  await page.screenshot({ path: test.info().outputPath('aspiration.png') });
  await expect(page.locator('.ecrire-vole')).toHaveCount(0);
  await expect.poll(() => calls(page)).toContainEqual(['envoyer_texte', { texte: 'où est géré le refresh token ?' }]);
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'think');
  // La coque annonce Écoute puis Réfléchit : la question ne ressort pas.
  await emit(page, 'state', { etat: 'assistant', activite: 'ecoute', mascotte: 'listen', transcrit: false });
  await emit(page, 'state', { etat: 'assistant', activite: 'reflechit', mascotte: 'think', transcrit: false });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'think');
  await expect(page.locator('[data-card=question]')).toHaveCount(0);
  // La réponse sort dans sa bulle habituelle ; la bulle d'écriture reste ouverte (conversation).
  await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'answer', 'Dans api/src/auth/refresh.ts, ligne 88.');
  await expect(page.locator('[data-card=answer] .text')).toHaveText('Dans api/src/auth/refresh.ts, ligne 88.');
  await expect(bulle(page)).toBeVisible();
  await page.screenshot({ path: test.info().outputPath('reponse.png') });
  // Un texte vide n'envoie rien.
  await page.keyboard.press('Enter');
  // Contrôle négatif : 600 ms laissés à un envoi pour partir.
  await page.waitForTimeout(600);
  expect((await calls(page)).filter(([c]) => c === 'envoyer_texte')).toHaveLength(1);
  await expectNoErrors(errors);
});

test('conversation : la bulle reste ouverte entre deux échanges et se range seule à la fin', async ({ page }) => {
  await mockTauri(page);
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.ecrire());
  await page.clock.runFor(1_000);
  await page.keyboard.type('bonjour');
  await page.keyboard.press('Enter');
  await page.clock.runFor(1_000);
  await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'state', { etat: 'assistant', activite: null, mascotte: 'idle', transcrit: false });
  // Bien plus de 8 s entre deux échanges : la conversation continue, la bulle reste.
  await page.clock.runFor(20_000);
  await expect(bulle(page)).toBeVisible();
  // Fin de la conversation (la coque repasse en veille) : elle se range.
  await emit(page, 'state', { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false });
  await page.clock.runFor(1_000);
  await expect(bulle(page)).toBeHidden();
  await expectNoErrors(errors);
});

test('Échap : la bulle se dégonfle et rentre dans Kobi', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.ecrire());
  await opened(page);
  await page.keyboard.type('finalement non');
  await expect(page.locator('.ecrire-c.nouv')).toHaveCount(0);
  const mouth = (await page.locator('.kobi .mouth').boundingBox())!;
  await page.keyboard.press('Escape');
  await expect(bulle(page)).toHaveAttribute('data-etat', 'fermeture');
  // Vers la fin, elle est minuscule, près de la bouche.
  const end = await bulle(page).evaluate((n) => {
    const a = n.getAnimations()[0]!;
    a.pause();
    a.currentTime = 379;
    const r = n.getBoundingClientRect();
    a.play();
    return { w: r.width, right: r.right };
  });
  expect(end.w).toBeLessThan(60);
  expect(Math.abs(end.right - (mouth.x + mouth.width / 2))).toBeLessThan(60);
  await expect(bulle(page)).toBeHidden();
  // Rouverte, elle repart vide.
  await page.evaluate(() => window.kobi.ecrire());
  await expect(champ(page)).toHaveValue('');
  await expectNoErrors(errors);
});

test('bulle vide pendant 8 s : elle se range seule ; du texte la garde ouverte', async ({ page }) => {
  await page.clock.install();
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.ecrire());
  await page.clock.runFor(7_000);
  await expect(bulle(page)).toBeVisible();
  await page.keyboard.type('a');
  await page.clock.runFor(10_000);
  await expect(bulle(page)).toBeVisible();
  await page.keyboard.press('Backspace');
  await page.clock.runFor(7_500);
  await expect(bulle(page)).toBeVisible();
  await page.clock.runFor(1_000);
  await expect(bulle(page)).toHaveAttribute('data-etat', /fermeture|rangee/);
  await page.clock.runFor(1_000);
  await expect(bulle(page)).toBeHidden();
  await expectNoErrors(errors);
});

test('réduire les animations : apparition et disparition en fondu court, rien ne bouge', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, 'reduced=1');
  // Ouverture et relevé dans la même tâche : sous charge, le fondu court est
  // fini avant un second aller-retour du test (KOB-172).
  const anims = await page.evaluate(() => {
    window.kobi.ecrire();
    const n = document.querySelector('form.ecrire')!;
    return n.getAnimations().map((a) => (a.effect as KeyframeEffect).getKeyframes().map((k) => Object.keys(k).filter((x) => !['offset', 'easing', 'composite', 'computedOffset'].includes(x))));
  });
  expect(anims).toEqual([[['opacity'], ['opacity']]]);
  await opened(page);
  await page.keyboard.type('bonjour');
  // Pas de brouillage, pas de mouvement.
  await expect(page.locator('.ecrire-c.nouv')).toHaveCount(0);
  const live = await page.locator('.ecrire').evaluate((n) => n.getAnimations({ subtree: true }).filter((a) => a.playState === 'running').length);
  expect(live).toBe(0);
  await page.keyboard.press('Enter');
  await expect.poll(() => calls(page)).toContainEqual(['envoyer_texte', { texte: 'bonjour' }]);
  await expect(page.locator('.ecrire-vole')).toHaveCount(0);
  await emit(page, 'state', { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false });
  await page.keyboard.press('Escape');
  await expect(bulle(page)).toBeHidden();
  await expectNoErrors(errors);
});
