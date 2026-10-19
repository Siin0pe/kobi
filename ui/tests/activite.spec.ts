// Mode « je m'en occupe » et fil d'activité (KOB-152) : l'état `work` de la
// mascotte et les étapes de l'événement `activite` de la coque (simulée).
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi } from './helpers';

// Le fil d'activité sous Kobi est celui du widget « bulle détaillée » ; en
// widget simple (défaut, KOB-169), il passe dans la vue agrandie.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('kobi.affichage', JSON.stringify({ mode: 'detaille', auto: 'jamais' })));
});

const etape = (id: string, type: string, resume: string, tour = 1, chemin: string | null = null) => ({
  tour,
  id,
  outil: type,
  type,
  resume,
  chemin_relatif: chemin,
  classe: 'lecture',
  decision: null,
  at_ms: Date.now(),
});

const ETAPES = [
  etape('t1', 'recherche', 'Je cherche « fabrique_claude » dans kobi'),
  etape('t2', 'lecture', 'Je lis crates/kobi-app/src/chef.rs', 1, 'crates/kobi-app/src/chef.rs'),
  etape('t3', 'commande', 'Je lance git log -5'),
  etape('t4', 'mcp', 'J’interroge Linear : list issues (state In Progress)'),
];

const WORK = { etat: 'veille', activite: 'occupe', mascotte: 'work', transcrit: false };

/** Ligne courte montrée sous Kobi : un chemin s'y réduit au nom du fichier. */
const COURTES = [
  'Je cherche « fabrique_claude »',
  'Je lis chef.rs',
  'Je lance git log -5',
  'J’interroge Linear : list issues (state In Progress)',
];

/** Textes posés des lignes visibles (hors lignes qui s'en vont). */
function lignes(page: Page) {
  return page.locator('.fil .step:not(.leaving) .step-text');
}

test('Je m’en occupe : une seule ligne sous Kobi, l’étape en cours, qui se remplace', async ({ page }) => {
  const errors = await openUi(page, 'backdrop=1');
  await page.evaluate(() => window.kobi.setState('work'));
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'work');
  await expect(page.locator('.kobi')).toHaveClass(/m-work/);
  const fil = page.getByRole('log', { name: 'Ce que fait Kobi' });
  for (const [i, e] of ETAPES.entries()) {
    await page.evaluate((e) => window.kobi.activite(e as never), e);
    await expect(fil).toBeVisible();
    // La mascotte prend le geste de l'outil.
    await expect(page.locator('.kobi')).toHaveAttribute('data-outil', e.type);
    await expect(lignes(page)).toHaveText([COURTES[i]!]);
    // Le résumé complet reste en info-bulle.
    await expect(page.locator('.fil .step:not(.leaving)')).toHaveAttribute('title', e.resume);
  }
  // L'étape remplacée s'efface : il ne reste qu'une ligne, sans carte ni fond.
  await expect(page.locator('.fil .step')).toHaveCount(1);
  const fond = await page.locator('.fil').evaluate((n) => getComputedStyle(n).backgroundColor);
  expect(fond).toBe('rgba(0, 0, 0, 0)');
  // Kobi ne se fige jamais : la pilule et l'icône de la dernière étape bougent.
  const anims = await page.evaluate(() => ({
    pill: document.querySelector('.kobi .pill')!.getAnimations().filter((a) => a.playState === 'running').length,
    icon: document.querySelector('.fil .step:last-child .step-icon')!.getAnimations().filter((a) => a.playState === 'running').length,
  }));
  expect(anims.pill).toBeGreaterThan(0);
  expect(anims.icon).toBeGreaterThan(0);
  await expectNoErrors(errors);
});

test('le texte d’une étape arrive brouillé puis se pose', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('work'));
  const texte = 'Je cherche « fabrique_claude » dans kobi';
  const vus = await page.evaluate(
    (texte) =>
      new Promise<string[]>((resolve) => {
        window.kobi.activite({ tour: 1, id: 'x', outil: 'lecture', type: 'lecture', resume: texte, chemin_relatif: null, classe: 'lecture', at_ms: Date.now() });
        const node = document.querySelector('.fil .step:last-child .step-text')!;
        const out = [node.textContent ?? ''];
        const t0 = performance.now();
        const tick = () => {
          out.push(node.textContent ?? '');
          if (performance.now() - t0 < 900) requestAnimationFrame(tick);
          else resolve(out);
        };
        requestAnimationFrame(tick);
      }),
    texte,
  );
  expect(vus[0]).not.toBe(texte);
  expect(vus.at(-1)).toBe(texte);
  await expectNoErrors(errors);
});

test('réduire les animations : même texte, sans mouvement ni brouillage', async ({ page }) => {
  const errors = await openUi(page, 'reduced=1');
  await page.evaluate(() => window.kobi.setState('work'));
  for (const e of ETAPES) await page.evaluate((e) => window.kobi.activite(e as never), e);
  // Pas de brouillage : le texte est posé tout de suite, les anciennes lignes partent sans glisser.
  await expect(page.locator('.fil .step .step-text')).toHaveText([COURTES[3]!]);
  const running = await page.evaluate(() =>
    document
      .querySelector('.fil')!
      .getAnimations({ subtree: true })
      .filter((a) => a.playState === 'running').length,
  );
  expect(running).toBe(0);
  await expect(page.locator('.kobi')).toHaveAttribute('data-outil', 'mcp');
  await expectNoErrors(errors);
});

test('un nouveau tour repart d’un fil vide ; hors travail, le fil se cache', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('work'));
  await page.evaluate((e) => window.kobi.activite(e as never), ETAPES[0]);
  await page.evaluate((e) => window.kobi.activite(e as never), etape('u1', 'lecture', 'Je lis README.md', 2, 'README.md'));
  await expect(lignes(page)).toHaveText(['Je lis README.md']);
  await page.evaluate(() => window.kobi.setState('answer', { answer: 'Dans chef.rs.' }));
  await expect(page.getByRole('log', { name: 'Ce que fait Kobi' })).toBeHidden();
  await page.evaluate(() => window.kobi.setState('work'));
  await expect(page.getByRole('log', { name: 'Ce que fait Kobi' })).toBeVisible();
  // En revenant au travail, Kobi reprend le geste du dernier outil.
  await expect(page.locator('.kobi')).toHaveAttribute('data-outil', 'lecture');
  await expectNoErrors(errors);
});

test('coque : l’événement activite s’affiche en moins de 300 ms et le délai est renvoyé', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'state', WORK);
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'work');
  const delais: number[] = [];
  for (const e of ETAPES) {
    const ms = await page.evaluate(
      (e) =>
        new Promise<number>((resolve) => {
          const t0 = performance.now();
          (window as unknown as { __emit(n: string, p: unknown): void }).__emit('activite', e);
          const check = () => {
            const last = document.querySelector('.fil .step:last-child');
            if (last?.getAttribute('data-id') === e.id) requestAnimationFrame(() => resolve(performance.now() - t0));
            else requestAnimationFrame(check);
          };
          check();
        }),
      e,
    );
    delais.push(ms);
  }
  for (const ms of delais) expect(ms).toBeLessThan(300);
  await expect
    .poll(async () => (await calls(page)).filter(([c]) => c === 'activite_affichee').map(([, a]) => a?.id))
    .toEqual(ETAPES.map((e) => e.id));
  const atMs = (await calls(page)).find(([c]) => c === 'activite_affichee')![1]!.atMs;
  expect(atMs).toBe(ETAPES[0]!.at_ms);
  await expectNoErrors(errors);
});

test('tâche longue : pas de notification en double pendant le travail, étapes repliées dans l’historique', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const bulle = (statut: string, etapes: unknown[], reponse = '') => [
    { tour: 1, question: 'où est fabrique_claude ?', reponse, statut, clavier: false, etapes },
  ];
  await emit(page, 'conversation', bulle('attente', []));
  await emit(page, 'state', WORK);
  await emit(page, 'conversation', bulle('tache', ETAPES.slice(0, 1)));
  await emit(page, 'activite', ETAPES[0]);
  // Une seule chose à la fois : la ligne sous Kobi, pas la notification.
  await expect(lignes(page)).toHaveText([COURTES[0]!]);
  await expect(page.locator('.notif-running')).toBeHidden();
  // Kobi passe à autre chose (une autre question) : la tâche en cours se rappelle.
  await emit(page, 'state', { ...WORK, activite: 'ecoute', mascotte: 'listen' });
  await expect(page.locator('.notif-running')).toBeVisible();
  await expect(page.locator('.notif-running .notif-steps')).toHaveCount(0);
  await emit(page, 'conversation', bulle('fini', ETAPES, 'Dans crates/kobi-app/src/chef.rs, ligne 910.'));
  // Historique : les étapes sont repliées sous la réponse, et se déplient.
  await page.evaluate(() => ((document.querySelector('.card-history') as HTMLElement).hidden = false));
  const steps = page.locator('.ex[data-tour="1"] .ex-steps');
  await expect(steps.locator('summary')).toHaveText('4 étapes');
  await expect(steps.locator('.step').first()).toBeHidden();
  await steps.locator('summary').click();
  await expect(steps.locator('.step-text')).toHaveText(ETAPES.map((e) => e.resume));
  await expectNoErrors(errors);
});

test('résultat d’une tâche montré dans la bulle : pas de notification « Résultat prêt » en plus', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const bulle = (statut: string, reponse = '') => [{ tour: 1, question: 'où est fabrique_claude ?', reponse, statut, clavier: false, etapes: ETAPES }];
  await emit(page, 'conversation', bulle('attente'));
  await emit(page, 'conversation', bulle('tache'));
  await emit(page, 'state', WORK);
  // La coque publie l'historique puis montre le résultat dans la bulle.
  await emit(page, 'conversation', bulle('fini', 'Dans chef.rs, ligne 910.'));
  await emit(page, 'state', { ...WORK, activite: 'repond', mascotte: 'answer' });
  await emit(page, 'answer', 'Dans chef.rs, ligne 910.');
  await expect(page.locator('[data-card=answer] .text')).toHaveText('Dans chef.rs, ligne 910.');
  await expect(page.locator('.notif')).toHaveCount(0);
  await expectNoErrors(errors);
});
