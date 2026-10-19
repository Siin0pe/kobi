// L'app Kobi (KOB-159) : une seule fenêtre, ses sections et leur adresse.
// Fausse coque Tauri ; l'ouverture côté Rust est testée dans app.rs.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors } from './helpers';

function fakeTauri(): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  const rappels = [
    { id: 'r1', texte: 'Appeler Léa', echeance_ms: Date.now() + 3_600_000, cree_ms: Date.now(), fait: false, source: null },
    { id: 'r2', texte: 'Envoyer le devis', echeance_ms: null, cree_ms: Date.now(), fait: true, source: null },
  ];
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        switch (cmd) {
          case 'carnet_index':
            return { racine: '/home/test/Kobi', reunions: [], conversations: [], rappels };
          case 'carnet_rappel_fait':
            rappels.find((r) => r.id === args.id)!.fait = true;
            return true;
          case 'reglages_stockage':
            return {
              dossier: '/home/test/Kobi',
              par_defaut: true,
              retention_reunions: 'unlimited',
              retention_conversations: 'unlimited',
              publication_reunions: { etat: 'aucune' },
              publication_conversations: { etat: 'aucune' },
            };
          case 'a_propos':
            return { version: '0.1.0', dossier: '/home/test/Kobi' };
          case 'app_section_demandee':
            return (w.__sectionDemandee as string | undefined) ?? null;
          case 'historique':
            return [
              { tour: 1, question: 'quelle heure est-il ?', reponse: '14 h 05.', statut: 'fini', clavier: false, etapes: [] },
              {
                tour: 2,
                question: 'où est fabrique_claude ?',
                reponse: 'Dans chef.rs.',
                statut: 'fini',
                clavier: true,
                etapes: [
                  { tour: 2, id: 't1', outil: 'recherche', type: 'recherche', resume: 'Je cherche « fabrique_claude » dans kobi', chemin_relatif: 'kobi', classe: 'lecture', at_ms: 0 },
                  { tour: 2, id: 't2', outil: 'lecture', type: 'lecture', resume: 'Je lis crates/kobi-app/src/chef.rs', chemin_relatif: 'crates/kobi-app/src/chef.rs', classe: 'lecture', at_ms: 0 },
                ],
              },
            ];
          case 'etat_session':
            return { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false };
          case 'etat_enregistrement':
            return { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null };
          case 'journal_activite':
            return [
              { quand_ms: Date.now() - 86_400_000, genre: 'reunion', texte: 'Réunion terminée', detail: 'Point hebdo · 4 lignes de notes' },
              { quand_ms: Date.now() - 60_000, genre: 'question', texte: 'Quelle heure est-il ?' },
              { quand_ms: Date.now() - 30_000, genre: 'action', texte: 'Proposé : cargo test', detail: 'Commande' },
            ];
          default:
            // Les étapes de l'accueil ont leurs propres tests (bienvenue.spec.ts).
            throw `Command ${cmd} not found`;
        }
      },
    },
    event: {
      listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
        (listeners[event] ??= []).push(handler);
        return () => {};
      },
    },
  };
}

async function ouvrirApp(page: Page, ancre = ''): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto(`/app.html${ancre}`);
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

const visible = (page: Page) => page.locator('.vue:not([hidden])');

test('navigation entre toutes les sections, adresse dans l’URL', async ({ page }) => {
  const errors = await ouvrirApp(page);
  // Par défaut : Notes et réunions (le Carnet).
  await expect(visible(page)).toHaveAttribute('data-section', 'notes');
  await expect(page.locator('.vue-notes .carnet')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Notes et réunions' })).toHaveAttribute('aria-current', 'page');
  for (const [nom, id] of [
    ['Rappels et tâches', 'rappels'],
    ['Activité', 'activite'],
    ['Réglages', 'reglages'],
    ['À propos', 'a-propos'],
    ['Notes et réunions', 'notes'],
  ] as const) {
    await page.getByRole('button', { name: nom, exact: true }).click();
    await expect(visible(page)).toHaveAttribute('data-section', id);
    await expect(page).toHaveURL(new RegExp(`#${id}`));
    await expect(page.getByRole('button', { name: nom, exact: true })).toHaveAttribute('aria-current', 'page');
  }
  await expect(page).toHaveTitle('Kobi · Notes et réunions');
  await expectNoErrors(errors);
});

test('app::ouvrir : adresse de départ et événement app://section', async ({ page }) => {
  const errors = await ouvrirApp(page, '#reglages/stockage');
  await expect(visible(page)).toHaveAttribute('data-section', 'reglages');
  await expect(page.locator('.vue-reglages .titre')).toHaveText('Stockage');
  // L'app est déjà ouverte : Rust envoie la section voulue.
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('app://section', 'rappels'));
  await expect(visible(page)).toHaveAttribute('data-section', 'rappels');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('app://section', 'a-propos'));
  await expect(page.locator('.vue-a-propos .infos')).toContainText('0.1.0');
  await expectNoErrors(errors);
});

test('KOB-189 : section demandée pendant le chargement de la page, pas perdue', async ({ page }) => {
  // `kobi --app` puis `kobi --reglages` aussitôt : l'événement app://section
  // part avant que la page l'écoute ; Rust garde l'adresse, la page la lit.
  await page.addInitScript(() => {
    (window as unknown as { __sectionDemandee: string }).__sectionDemandee = 'reglages/stockage';
  });
  const errors = await ouvrirApp(page, '#notes');
  await expect(visible(page)).toHaveAttribute('data-section', 'reglages');
  await expect(page.locator('.vue-reglages .titre')).toHaveText('Stockage');
  await expect(page).toHaveURL(/#reglages\/stockage/);
  await expectNoErrors(errors);
});

test('premier démarrage : l’accueil occupe toute la fenêtre, puis l’app', async ({ page }) => {
  await ouvrirApp(page, '#accueil');
  await expect(page.locator('.vue-accueil')).toBeVisible();
  await expect(page.locator('.vue-accueil .titre')).toHaveText('Salut, moi c’est Kobi.');
  await expect(page.locator('.nav')).toBeHidden();
  // Quitter l'accueil par la navigation (adresse) : la nav revient.
  await page.evaluate(() => window.kobiApp.aller('notes'));
  await expect(page.locator('.nav')).toBeVisible();
  await expect(page.locator('.vue-accueil')).toBeHidden();
  await expect(visible(page)).toHaveAttribute('data-section', 'notes');
});

test('rappels et tâches : à venir, faits, et cocher', async ({ page }) => {
  const errors = await ouvrirApp(page, '#rappels');
  await expect(page.locator('.vue-rappels .groupe')).toHaveText(['À venir (1)', 'Faits (1)']);
  await page.getByRole('button', { name: 'Marquer « Appeler Léa » comme fait' }).click();
  await expect(page.locator('.notif')).toHaveText('Rappel fait.');
  await expect(page.locator('.vue-rappels .groupe')).toHaveText(['À venir (0)', 'Faits (2)']);
  await expectNoErrors(errors);
});

test('un écran s’ajoute aux Réglages sans toucher à l’app (ajouterPanneau)', async ({ page }) => {
  const errors = await ouvrirApp(page);
  await page.evaluate(() =>
    window.kobiApp.ajouterPanneau(
      {
        id: 'mcp',
        titre: 'Outils (MCP)',
        sousTitre: 'Connecter tes outils',
        monter: (el: HTMLElement) => void el.replaceChildren(Object.assign(document.createElement('p'), { textContent: 'Panneau ajouté.' })),
      },
      'stockage',
    ),
  );
  await page.evaluate(() => window.kobiApp.aller('reglages/mcp'));
  await expect(page.locator('.vue-reglages .titre')).toHaveText('Outils (MCP)');
  await expect(page.locator('.vue-reglages .panneau')).toHaveText('Panneau ajouté.');
  // Rangé juste après Stockage dans le menu.
  await expect(page.locator('.vue-reglages .menu-entree').nth(1)).toHaveAttribute('data-panneau', 'mcp');
  await expectNoErrors(errors);
});

test('Activité : les étapes des derniers échanges (KOB-152), à jour en direct', async ({ page }) => {
  const errors = await ouvrirApp(page, '#activite');
  const journal = page.getByRole('log', { name: 'Étapes des derniers échanges' });
  // Seuls les échanges qui ont des étapes, le plus récent d'abord.
  await expect(journal.locator('.journal-question')).toHaveText(['où est fabrique_claude ?']);
  await expect(journal.locator('.step-text')).toHaveText(['Je cherche « fabrique_claude » dans kobi', 'Je lis crates/kobi-app/src/chef.rs']);
  await page.evaluate(() =>
    (window as unknown as { __emit(e: string, p: unknown): void }).__emit('conversation', [
      {
        tour: 3,
        question: 'et les tests ?',
        reponse: '',
        statut: 'tache',
        clavier: false,
        etapes: [{ tour: 3, id: 'u1', outil: 'recherche', type: 'recherche', resume: 'Je parcours kobi/ui/tests/**/*.ts', chemin_relatif: null, classe: 'lecture', at_ms: 0 }],
      },
    ]),
  );
  await expect(journal.locator('.journal-question')).toHaveText(['et les tests ?']);
  await expectNoErrors(errors);
});

test('le petit Kobi de la navigation suit la mascotte', async ({ page }) => {
  const errors = await ouvrirApp(page, '#rappels');
  const kobi = page.locator('.nav-kobi');
  const emettre = (etat: string) =>
    page.evaluate((m) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('state', { etat: 'assistant', activite: null, mascotte: m, transcrit: false }), etat);
  // Au repos : le mot de la section.
  await expect(kobi).toHaveAttribute('data-etat', 'idle');
  await expect(kobi.locator('.mot')).toHaveText('rappels');
  for (const [etat, oeil, mot] of [
    ['listen', 'O', 'oui ?'],
    ['think', '@', 'hmm…'],
    ['answer', '^', 'alors'],
    ['error', 'x', 'oups'],
  ] as const) {
    await emettre(etat);
    await expect(kobi).toHaveAttribute('data-etat', etat);
    await expect(kobi.locator('.g').first()).toHaveText(oeil);
    await expect(kobi.locator('.mot')).toHaveText(mot);
  }
  // Occupé, il garde son mot quand on change de section ; au repos, il reprend celui de la section.
  await page.getByRole('button', { name: 'Réglages' }).click();
  await expect(kobi.locator('.mot')).toHaveText('oups');
  await emettre('idle');
  await expect(kobi.locator('.mot')).toHaveText('réglages');
  // En réunion, au calme : il prend des notes, comme la mascotte.
  await page.evaluate(() =>
    (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('state', { etat: 'reunion', activite: 'calme', mascotte: 'idle', transcrit: true }),
  );
  await expect(kobi).toHaveAttribute('data-etat', 'write');
  await expect(kobi.locator('.mot')).toHaveText('noté');
  await expectNoErrors(errors);
});

test('Activité : le journal, du plus récent au plus ancien, et les nouvelles entrées en direct', async ({ page }) => {
  const errors = await ouvrirApp(page, '#activite');
  const entrees = page.locator('.vue-activite .fil .entree');
  await expect(entrees).toHaveCount(3);
  await expect(entrees.locator('.entree-texte')).toHaveText(['Proposé : cargo test', 'Quelle heure est-il ?', 'Réunion terminée']);
  await expect(page.locator('.vue-activite .fil .groupe')).toHaveText(['Aujourd’hui', 'Hier']);
  await expect(entrees.nth(2).locator('.entree-meta')).toContainText('Point hebdo · 4 lignes de notes');
  await page.evaluate(() =>
    (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('journal', { quand_ms: Date.now(), genre: 'validation', texte: 'Accepté : cargo test' }),
  );
  await expect(entrees).toHaveCount(4);
  await expect(entrees.first()).toHaveAttribute('data-genre', 'validation');
  await expect(entrees.first()).toHaveClass(/nouvelle/);
  await expectNoErrors(errors);
});

test('la feuille de styles de l’app est lue jusqu’au bout (aucune accolade perdue)', async ({ page }) => {
  const errors = await ouvrirApp(page);
  const animations = await page.evaluate(() =>
    [...document.styleSheets].flatMap((f) => [...f.cssRules].filter((r) => r instanceof CSSKeyframesRule).map((r) => (r as CSSKeyframesRule).name)),
  );
  // Les dernières de app.css : une accolade perdue plus haut les ferait disparaître.
  expect(animations).toEqual(expect.arrayContaining(['app-pulse-rec', 'app-ecoute', 'app-bandeau', 'app-nouvelle', 'app-roule', 'app-bandeau-part']));
  await expectNoErrors(errors);
});
