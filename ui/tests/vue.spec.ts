// Widget simple par défaut et vue agrandie de la conversation (KOB-169) :
// question courte dans le widget, réponse longue ouverte en grand tout seul,
// ouverture à la demande (clic, voix ou raccourci via la coque, commande
// kobi), fermeture (Échap, clic à côté, délai), réglages appliqués à chaud.
// La coque simulée agrandit la fenêtre (ici la vue du navigateur).
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

test.use({ video: { mode: 'on', size: { width: 820, height: 560 } } });

const vue = (page: Page) => page.getByRole('dialog', { name: 'Conversation avec Kobi' });
const carte = (page: Page) => page.locator('[data-card=answer]');
const PHRASE = 'le verrou protège la donnée partagée pendant que chaque fil attend son tour.';
const longue = (n: number) => Array.from({ length: n }, () => PHRASE).join(' ');
const RICHE = ['Un **mutex** protège la donnée :', '', '- on prend le verrou avec `lock()` ;', '- on le relâche en fin de bloc.', '', '```rust', 'let n = m.lock().unwrap();', '```'].join('\n');

/** Fenêtre de Kobi 410 × 280, agrandie par la coque simulée quand l'interface le demande. */
async function kobi(page: Page, reglages?: Record<string, unknown>, query = ''): Promise<string[]> {
  await page.setViewportSize({ width: 410, height: 280 });
  await page.exposeFunction('__resize', (w: number, h: number) => page.setViewportSize({ width: w, height: h }));
  await mockTauri(page);
  if (reglages) await page.addInitScript((r) => localStorage.setItem('kobi.affichage', JSON.stringify({ v: 2, ...r })), reglages);
  await page.addInitScript(() => {
    const w = window as unknown as { __TAURI__: { core: { invoke: (c: string, a?: Record<string, number | string | boolean>) => Promise<unknown> } }; __resize(w: number, h: number): Promise<void> };
    const base = w.__TAURI__.core.invoke;
    w.__TAURI__.core.invoke = async (cmd, args) => {
      const out = await base(cmd, args);
      if (cmd !== 'agrandir_kobi') return out;
      if (!args?.grand) {
        if (args?.appliquer) await w.__resize(410, 280);
        return null;
      }
      // Panneau à gauche de Kobi (« à côté »), Kobi dans le coin bas droit.
      const lp = Number(args.largeur), hp = Number(args.hauteur);
      const W = lp + 230, H = Math.max(hp + 32, 280);
      if (args.appliquer) await w.__resize(W, H);
      return { kobi: { x: W - 410, y: H - 280, largeur: 410, hauteur: 280 }, panneau: { x: 16, y: H - 16 - hp, largeur: lp, hauteur: hp } };
    };
  });
  return openUi(page, query);
}

async function repond(page: Page, question: string, reponse: string): Promise<void> {
  await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'question', question);
  await emit(page, 'answer', reponse);
}

const pill = async (page: Page) => {
  // Boucles de Kobi (respiration…) ramenées à leur première image : la mesure
  // ne dépend plus de la phase où elle tombe (comme KOB-172).
  await page.evaluate(() => {
    for (const a of document.querySelector('.kobi')!.getAnimations({ subtree: true })) {
      if (a.effect?.getTiming().iterations === Infinity) a.currentTime = 0;
    }
  });
  const b = (await page.locator('.kobi .pill').boundingBox())!;
  const vp = page.viewportSize()!;
  return { right: Math.round(vp.width - (b.x + b.width)), bottom: Math.round(vp.height - (b.y + b.height)), w: Math.round(b.width) };
};

test('widget simple par défaut : une question courte reste dans la bulle, rien ne s’ouvre', async ({ page }) => {
  const errors = await kobi(page);
  await expect(page.locator('.stage')).toHaveClass(/affichage-simple/);
  await repond(page, 'quelle heure est-il ?', 'Il est 10 h 12.');
  await expect(carte(page).locator('.text')).toHaveText('Il est 10 h 12.', { timeout: 5_000 });
  await page.waitForTimeout(600);
  await expect(vue(page)).toBeHidden();
  expect((await calls(page)).filter(([c]) => c === 'agrandir_kobi')).toHaveLength(0);
  await page.screenshot({ path: test.info().outputPath('widget-court.png') });
  await expectNoErrors(errors);
});

test('réponse longue : la bulle en montre le début (3 lignes), la vue agrandie s’ouvre seule à côté de Kobi, qui ne bouge pas', async ({ page }) => {
  // L'ouverture seule est un réglage (KOB-215 : par défaut, à la demande).
  const errors = await kobi(page, { auto: 'longues' });
  await page.waitForTimeout(300);
  const avant = await pill(page);
  await repond(page, 'explique le mutex', `${RICHE}\n\n${longue(20)}`);
  await expect(vue(page)).toBeVisible({ timeout: 8_000 });
  await expect.poll(() => page.viewportSize()!.width).toBe(640 + 230);
  await page.waitForTimeout(800);
  // Kobi : même place, même taille.
  const apres = await pill(page);
  for (const k of ['right', 'bottom', 'w'] as const) expect(Math.abs(apres[k] - avant[k]), k).toBeLessThanOrEqual(2);
  // La vue : texte à 18 px réels, mis en forme, sans recouvrir Kobi, dans la fenêtre.
  const v = (await vue(page).boundingBox())!;
  const k = (await page.locator('.kobi .pill').boundingBox())!;
  expect(v.x + v.width).toBeLessThanOrEqual(k.x + 1);
  expect(v.y).toBeGreaterThanOrEqual(0);
  await expect(vue(page).locator('.vex-q')).toHaveText('explique le mutex');
  await expect(vue(page).locator('ul > li')).toHaveCount(2);
  await expect(vue(page).locator('pre code')).toContainText('let n = m.lock().unwrap();');
  expect(await vue(page).evaluate((n) => parseFloat(getComputedStyle(n).fontSize))).toBe(18);
  // Sobriété : dans la scène, la bulle de réponse s'efface, la conversation se lit dans la vue.
  await expect(carte(page)).toBeHidden();
  expect(await calls(page)).toContainEqual(['agrandir_kobi', { grand: true, appliquer: true, largeur: 640, hauteur: 560, position: 'a-cote', raison: 'réponse longue' }]);
  await page.screenshot({ path: test.info().outputPath('vue-auto.png') });
  // Échap la range ; la fenêtre reprend sa taille, Kobi n'a pas bougé.
  await page.keyboard.press('Escape');
  await expect(vue(page)).toBeHidden();
  await expect.poll(() => page.viewportSize()!.width).toBe(410);
  // Rangée par l'utilisateur : elle ne se rouvre pas seule pendant la même réponse.
  await emit(page, 'answer', `${RICHE}

${longue(24)}`);
  await page.waitForTimeout(800);
  await expect(vue(page)).toBeHidden();
  const fin = await pill(page);
  for (const x of ['right', 'bottom', 'w'] as const) expect(Math.abs(fin[x] - avant[x]), x).toBeLessThanOrEqual(2);
  await expect(page.locator('.stage')).not.toHaveClass(/agrandie/);
  await expectNoErrors(errors);
});

test('widget simple : la bulle est limitée à trois lignes, « Lire en grand » et un clic sur la bulle ouvrent la vue', async ({ page }) => {
  const errors = await kobi(page, { auto: 'jamais' });
  await repond(page, 'raconte', longue(12));
  await page.waitForTimeout(1_500);
  const z = await carte(page).locator('.defile').evaluate((n) => ({ h: n.clientHeight, l: parseFloat(getComputedStyle(n).lineHeight), t: n.scrollTop, s: n.scrollHeight }));
  expect(z.h).toBeLessThanOrEqual(z.l * 3 + 14);
  expect(z.t).toBe(0);
  expect(z.s).toBeGreaterThan(z.h);
  await expect(vue(page)).toBeHidden();
  await page.screenshot({ path: test.info().outputPath('widget-trois-lignes.png') });
  await carte(page).getByRole('button', { name: 'Lire en grand ›' }).click();
  await expect(vue(page)).toBeVisible();
  await expect(vue(page).locator('.vex-a')).toContainText('chaque fil attend son tour');
  // Un clic à côté du panneau (zone transparente de la fenêtre agrandie) la range.
  await expect.poll(() => page.viewportSize()!.width).toBe(870);
  await page.mouse.click(page.viewportSize()!.width - 30, 10);
  await expect(vue(page)).toBeHidden();
  await expect.poll(() => page.viewportSize()!.width).toBe(410);
  // Un clic sur la bulle elle-même l'ouvre aussi.
  await carte(page).click();
  await expect(vue(page)).toBeVisible();
  await expectNoErrors(errors);
});

test('widget simple : le fil d’activité passe dans la vue (étapes repliées), les notifications restent', async ({ page }) => {
  const errors = await kobi(page, { auto: 'jamais' });
  await emit(page, 'state', { etat: 'assistant', activite: 'travaille', mascotte: 'work', transcrit: false });
  await emit(page, 'activite', { tour: 1, id: 'a1', outil: 'Grep', type: 'recherche', resume: 'cherche « refresh »', chemin_relatif: null, classe: 'lecture', at_ms: Date.now() });
  await page.waitForTimeout(400);
  await expect(page.locator('.fil')).toBeHidden();
  const etape = { tour: 1, id: 'a1', outil: 'Grep', type: 'recherche', resume: 'cherche « refresh »', chemin_relatif: null, classe: 'lecture', at_ms: Date.now() };
  await emit(page, 'conversation', [{ tour: 1, question: 'où est le refresh ?', reponse: 'Dans auth.ts.', statut: 'fini', clavier: true, etapes: [etape] }]);
  await emit(page, 'vue', { action: 'ouvrir' });
  await expect(vue(page).locator('.vex-etapes summary')).toHaveText('1 étape');
  await expectNoErrors(errors);
});

test('à la demande : « Kobi, montre en grand », le raccourci et `kobi view` passent par l’événement vue de la coque', async ({ page }) => {
  const errors = await kobi(page, { auto: 'jamais' });
  await repond(page, 'bonjour', 'Bonjour !');
  await emit(page, 'vue', { action: 'ouvrir' });
  await expect(vue(page)).toBeVisible();
  await expect(vue(page).locator('.vex-a')).toHaveText('Bonjour !');
  await emit(page, 'vue', { action: 'basculer' });
  await expect(vue(page)).toBeHidden();
  await emit(page, 'vue', { action: 'basculer' });
  await expect(vue(page)).toBeVisible();
  await emit(page, 'vue', { action: 'fermer' });
  await expect(vue(page)).toBeHidden();
  await expectNoErrors(errors);
});

test('la vue se range seule après le délai réglé une fois la réponse finie ; la souris dessus la retient', async ({ page }) => {
  await page.clock.install();
  const errors = await kobi(page, { auto: 'toujours', delai: 10 });
  await repond(page, 'bonjour', 'Bonjour, je suis là.');
  await page.clock.runFor(2_000);
  await expect(vue(page)).toBeVisible();
  // Kobi répond encore : elle reste.
  await page.clock.runFor(15_000);
  await expect(vue(page)).toBeVisible();
  await emit(page, 'state', { etat: 'assistant', activite: null, mascotte: 'idle', transcrit: false });
  // La souris dessus : elle reste.
  const v = (await vue(page).boundingBox())!;
  await page.mouse.move(v.x + v.width / 2, v.y + v.height / 2);
  await page.clock.runFor(12_000);
  await expect(vue(page)).toBeVisible();
  await page.mouse.move(page.viewportSize()!.width - 5, 5);
  await page.clock.runFor(11_000);
  await expect(vue(page)).toBeHidden();
  await expectNoErrors(errors);
});

test('réglages changés dans l’app (autre fenêtre) : appliqués tout de suite, sans redémarrer', async ({ page, context }) => {
  const errors = await kobi(page, { auto: 'jamais' });
  await repond(page, 'explique', RICHE);
  await emit(page, 'vue', { action: 'ouvrir' });
  await expect(vue(page)).toBeVisible();
  await expect.poll(() => page.viewportSize()!.width).toBe(870);
  // Une autre fenêtre de Kobi (l'app) change les réglages.
  const app = await context.newPage();
  await app.goto('/?backdrop=0');
  await app.evaluate(() => localStorage.setItem('kobi.affichage', JSON.stringify({ mode: 'detaille', auto: 'jamais', taille: 'grande', position: 'centre', texte: 'grand', delai: 0 })));
  // Mode, taille du texte, taille et place de la vue : tout de suite, vue ouverte.
  await expect(page.locator('.stage')).toHaveClass(/affichage-detaille/);
  await expect.poll(() => vue(page).evaluate((n) => parseFloat(getComputedStyle(n).fontSize))).toBe(22);
  await expect.poll(async () => (await calls(page)).some(([c, a]) => c === 'agrandir_kobi' && a?.position === 'centre' && a?.largeur === 860 && a?.appliquer === true)).toBe(true);
  await expect.poll(() => page.viewportSize()!.width).toBe(860 + 230);
  await page.screenshot({ path: test.info().outputPath('reglages-a-chaud.png') });
  await app.close();
  await expectNoErrors(errors);
});

test('panneau « Affichage » de la section Réglages de l’app : chaque choix est enregistré', async ({ page }) => {
  await mockTauri(page);
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto('/app.html#reglages/affichage');
  const groupe = (nom: string) => page.getByRole('radiogroup', { name: nom });
  await expect(groupe('Widget de Kobi')).toBeVisible();
  await expect(groupe('Widget de Kobi').getByRole('radio', { name: /Simple/ })).toHaveAttribute('aria-checked', 'true');
  await groupe('Widget de Kobi').getByRole('radio', { name: /Bulle détaillée/ }).click();
  await groupe('Ouvrir la conversation en grand').getByRole('radio', { name: /Toujours/ }).click();
  await groupe('Taille de la vue').getByRole('radio', { name: /Grande/ }).click();
  await groupe('Place de la vue').getByRole('radio', { name: /Moitié droite/ }).click();
  await groupe('Taille du texte').getByRole('radio', { name: /Petit/ }).click();
  await groupe('Ranger la vue').getByRole('radio', { name: /Jamais/ }).click();
  await expect(groupe('Afficher l’état en texte').getByRole('radio', { name: /Oui/ })).toHaveAttribute('aria-checked', 'true');
  await groupe('Afficher l’état en texte').getByRole('radio', { name: /Non/ }).click();
  const r = await page.evaluate(() => JSON.parse(localStorage.getItem('kobi.affichage') ?? '{}'));
  expect(r).toEqual({ v: 2, mode: 'detaille', auto: 'toujours', taille: 'grande', position: 'moitie-droite', texte: 'petit', delai: 0, etat: 'aucun' });
  await page.screenshot({ path: test.info().outputPath('reglages-affichage.png') });
});

test('Réglages › Affichage › « Kobi parle » : normal par défaut, discret envoyé à la coque (KOB-215)', async ({ page }) => {
  await mockTauri(page, { verbosite_ui: 'normal', regler_verbosite_ui: 'discret' });
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto('/app.html#reglages/affichage');
  const groupe = page.getByRole('radiogroup', { name: 'Kobi parle' });
  await expect(groupe.getByRole('radio', { name: /Normal/ })).toHaveAttribute('aria-checked', 'true');
  await groupe.getByRole('radio', { name: /Discret/ }).click();
  await expect(groupe.getByRole('radio', { name: /Discret/ })).toHaveAttribute('aria-checked', 'true');
  await expect(groupe.getByRole('radio', { name: /Normal/ })).toHaveAttribute('aria-checked', 'false');
  expect((await calls(page)).filter(([c]) => c === 'regler_verbosite_ui')).toEqual([['regler_verbosite_ui', { verbosite: 'discret' }]]);
});

test('réduire les animations : la vue apparaît et disparaît en fondu court', async ({ page }) => {
  const errors = await kobi(page, { auto: 'jamais' }, 'reduced=1');
  await repond(page, 'bonjour', 'Bonjour !');
  await emit(page, 'vue', { action: 'ouvrir' });
  await expect(vue(page)).toBeVisible();
  const anims = await vue(page).evaluate((n) =>
    n.getAnimations().map((a) => (a.effect as KeyframeEffect).getKeyframes().flatMap((k) => Object.keys(k).filter((x) => !['offset', 'easing', 'composite', 'computedOffset'].includes(x)))),
  );
  for (const a of anims) expect(a.every((x) => x === 'opacity')).toBe(true);
  await page.keyboard.press('Escape');
  await expect(vue(page)).toBeHidden();
  await expectNoErrors(errors);
});

// KOB-177 : la réponse ne s'affiche qu'une fois dans la vue, en flux puis
// finie, même quand la coque publie la réponse finie rognée ou que la
// question n'est pas arrivée avant le flux.
for (const [cas, avecQuestion, fin] of [
  ['même texte', true, ''],
  ['réponse finie rognée par la coque', true, '\n\n'],
  ['flux sans question', false, ''],
] as const) {
  test(`une réponse n’apparaît qu’une fois dans la vue (${cas})`, async ({ page }) => {
    const errors = await kobi(page, { auto: 'toujours' });
    const question = 'explique le mutex';
    const reponse = longue(30);
    const flux = reponse + fin;
    await emit(page, 'state', { etat: 'assistant', activite: 'repond', mascotte: 'answer', transcrit: false });
    if (avecQuestion) await emit(page, 'question', question);
    for (let i = 200; i < flux.length; i += 400) await emit(page, 'answer', flux.slice(0, i));
    await emit(page, 'answer', flux);
    await expect(vue(page)).toBeVisible();
    await expect(vue(page).locator('.vex')).toHaveCount(1);
    // La coque publie l'historique (réponse finie), puis Kobi revient au calme.
    await emit(page, 'conversation', [{ tour: 1, question, reponse, statut: 'fini', clavier: false, etapes: [] }]);
    await emit(page, 'state', { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false });
    await expect(vue(page).locator('.vex')).toHaveCount(1);
    await expect(vue(page).locator('.vex .vex-q')).toHaveText(question);
    // Une deuxième question : deux échanges, chacun une fois.
    await repond(page, 'et le sémaphore ?', 'Un sémaphore compte les places libres.');
    await emit(page, 'conversation', [
      { tour: 1, question, reponse, statut: 'fini', clavier: false, etapes: [] },
      { tour: 2, question: 'et le sémaphore ?', reponse: 'Un sémaphore compte les places libres.', statut: 'fini', clavier: false, etapes: [] },
    ]);
    await expect(vue(page).locator('.vex')).toHaveCount(2);
    await expectNoErrors(errors);
  });
}

test('par défaut, rien ne s’ouvre seul : la réponse longue reste dans la bulle, « Lire en grand » l’ouvre (KOB-215)', async ({ page }) => {
  const errors = await kobi(page);
  await repond(page, 'explique le mutex', `${RICHE}\n\n${longue(20)}`);
  await expect(carte(page).getByText('Lire en grand')).toBeVisible();
  await page.waitForTimeout(800);
  await expect(vue(page)).toBeHidden();
  expect((await calls(page)).filter(([c]) => c === 'agrandir_kobi')).toEqual([]);
  await carte(page).getByText('Lire en grand').click();
  await expect(vue(page)).toBeVisible();
  await expectNoErrors(errors);
});

test('ancien format des réglages d’affichage : « réponses longues », l’ancien défaut, retombe sur « À la demande », les autres choix restent (KOB-215)', async ({ page }) => {
  await mockTauri(page);
  await page.addInitScript(() =>
    localStorage.setItem('kobi.affichage', JSON.stringify({ mode: 'detaille', auto: 'longues', taille: 'moyenne', position: 'a-cote', texte: 'grand', delai: 30 })),
  );
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto('/app.html#reglages/affichage');
  const groupe = (nom: string) => page.getByRole('radiogroup', { name: nom });
  await expect(groupe('Ouvrir la conversation en grand').getByRole('radio', { name: /À la demande/ })).toHaveAttribute('aria-checked', 'true');
  await expect(groupe('Widget de Kobi').getByRole('radio', { name: /Bulle détaillée/ })).toHaveAttribute('aria-checked', 'true');
  await expect(groupe('Taille du texte').getByRole('radio', { name: /Grand/ })).toHaveAttribute('aria-checked', 'true');
  // Un nouveau choix enregistre le format 2, sans les défauts.
  await groupe('Taille du texte').getByRole('radio', { name: /Petit/ }).click();
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem('kobi.affichage') ?? '{}'))).toEqual({ v: 2, mode: 'detaille', taille: 'moyenne', position: 'a-cote', texte: 'petit', delai: 30 });
});

test('ancien format : « Toujours », choisi exprès, reste', async ({ page }) => {
  await mockTauri(page);
  await page.addInitScript(() => localStorage.setItem('kobi.affichage', JSON.stringify({ mode: 'simple', auto: 'toujours' })));
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto('/app.html#reglages/affichage');
  await expect(page.getByRole('radiogroup', { name: 'Ouvrir la conversation en grand' }).getByRole('radio', { name: /Toujours/ })).toHaveAttribute('aria-checked', 'true');
});

// KOB-238 : la vue ne s'ouvre seule qu'une fois par conversation, et jamais pendant une rafale.
const veille = (page: Page) => emit(page, 'state', { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false });

test('ouverture seule : une fois par conversation, même si Kobi revient en veille entre deux réponses (KOB-238)', async ({ page }) => {
  const errors = await kobi(page, { auto: 'toujours', delai: 0 });
  await repond(page, 'q1', 'Première réponse.');
  await expect(vue(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(vue(page)).toBeHidden();
  // Comme en vrai hors réunion : chaque réponse finit en veille, la
  // conversation continue.
  await veille(page);
  await page.waitForTimeout(10_500);
  await repond(page, 'q2', 'Deuxième réponse, même conversation.');
  await page.waitForTimeout(800);
  await expect(vue(page)).toBeHidden();
  // La coque clôt la conversation : la suivante peut s'ouvrir seule.
  await veille(page);
  await emit(page, 'fin_conversation', null);
  await page.waitForTimeout(10_500);
  await repond(page, 'q3', 'Réponse d’une nouvelle conversation.');
  await expect(vue(page)).toBeVisible();
  await expectNoErrors(errors);
});

test('rafale : deux réponses à moins de 10 s, la seconde n’ouvre rien (KOB-238)', async ({ page }) => {
  const errors = await kobi(page, { auto: 'toujours', delai: 0 });
  await repond(page, 'q1', 'Première réponse.');
  await expect(vue(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await veille(page);
  await repond(page, 'q2', 'Deuxième réponse, deux secondes après.');
  await page.waitForTimeout(800);
  await expect(vue(page)).toBeHidden();
  expect((await calls(page)).filter(([c, a]) => c === 'agrandir_kobi' && (a as { grand?: boolean } | null)?.grand && (a as { appliquer?: boolean }).appliquer)).toHaveLength(1);
  await expectNoErrors(errors);
});

test('KOB-246 : la molette vers le haut sur la vue agrandie ne fait pas surgir l’historique caché dessous', async ({ page }) => {
  const errors = await kobi(page, { auto: 'toujours', delai: 0 });
  await repond(page, 'q1', 'Première réponse.');
  await repond(page, 'q2', `Deuxième réponse. ${'Une phrase pour remplir la vue. '.repeat(40)}`);
  await expect(vue(page)).toBeVisible();
  const box = (await vue(page).boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, -300);
  await page.waitForTimeout(500);
  await expect(page.getByRole('log', { name: 'Échanges récents avec Kobi' })).toBeHidden();
  await page.keyboard.press('Escape');
  await expect(vue(page)).toBeHidden();
  await page.waitForTimeout(500);
  await expect(page.getByRole('log', { name: 'Échanges récents avec Kobi' })).toBeHidden();
  await expectNoErrors(errors);
});
