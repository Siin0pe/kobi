// Le Carnet (KOB-140) avec une fausse coque Tauri : les commandes `carnet_*`
// répondent depuis un stockage en mémoire. Leur comportement réel est testé
// côté Rust (crates/kobi-app/src/carnet.rs, tests IPC).
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors } from './helpers';

function fakeTauri(): void {
  const H = 3_600_000;
  // Tout se passe aujourd'hui (9 h, 10 h, 10 h 30), quelle que soit l'heure du test.
  const aujourdhui = new Date();
  aujourdhui.setHours(9, 0, 0, 0);
  const debut = aujourdhui.getTime();
  const maintenant = debut + 2 * H;
  const reunions = [
    {
      nom: '2026-10-18_point-budget',
      date: '2026-10-18',
      titre: 'Point budget',
      debut_ms: debut,
      fin_ms: debut + 42 * 60_000,
      en_cours: false,
      duree_s: 2520,
      a_compte_rendu: true,
      apercu: 'Budget validé pour le prototype.',
    },
  ];
  // Réunion encore en cours (KOB-251) : commencée il y a 12 min, transcription
  // relue il y a 7 min (durée figée à 5 min).
  if ((window as unknown as { __enCours?: boolean }).__enCours) {
    Object.assign(reunions[0]!, { en_cours: true, debut_ms: Date.now() - 12 * 60_000, fin_ms: null, duree_s: 300, a_compte_rendu: false });
  }
  const lue = {
    resume: reunions[0],
    dossier: '/home/test/Kobi/reunions/2026-10-18_point-budget',
    segments: [
      { start: 0, end: 4, source: 'moi', text: 'Bonjour à tous, on parle du budget.', is_final: true },
      { start: 5.5, end: 9, source: 'eux', text: 'Le prototype de la fusée vole jeudi.', is_final: true },
      { start: 75, end: 80, source: 'moi', text: 'Parfait, on valide.', is_final: true },
    ],
    notes: '- Budget : 12 k€\n- Fusée prête jeudi',
    compte_rendu: {
      resume: 'Budget validé pour le prototype.',
      decisions: '- On garde la **fusée** v2',
      actions: '- [ ] Envoyer le devis\n- [x] Réserver la salle',
      questions: '',
      autres: [],
      brut: '',
    },
  };
  const conversations = [
    { id: '2026-10-18/150000', date: '2026-10-18', debut_ms: maintenant - H, fin_ms: maintenant - H + 60_000, tours: 2, apercu: 'Quelle heure est-il à Tokyo ?' },
    { id: '2026-10-18/160000', date: '2026-10-18', debut_ms: maintenant - H / 2, fin_ms: null, tours: 2, apercu: 'Où est géré le refresh token ?' },
  ];
  const tours: Record<string, unknown[]> = {
    '2026-10-18/150000': [
      { at_ms: maintenant - H, qui: 'moi', texte: 'Quelle heure est-il à Tokyo ?', voix: true },
      { at_ms: maintenant - H + 2000, qui: 'kobi', texte: 'Il est **23 h** à Tokyo.', voix: false },
    ],
    '2026-10-18/160000': [
      { at_ms: maintenant - H / 2, qui: 'moi', texte: 'Où est géré le refresh token ?', voix: false },
      { at_ms: maintenant - H / 2 + 3000, qui: 'kobi', texte: 'Dans `auth/token.rs`, ligne 42.', voix: false },
    ],
  };
  const rappels = [
    { id: 'r1', texte: 'Appeler Léa', echeance_ms: debut + 24 * H, cree_ms: maintenant - H, fait: false, source: null },
  ];
  let reglages: Record<string, unknown> = {
    dossier: '/home/test/Kobi',
    par_defaut: true,
    retention_reunions: 'unlimited',
    retention_conversations: 'unlimited',
    publication_reunions: { etat: 'reglee', mcp: 'linear', destination: 'Projet Kobi' },
    publication_conversations: { etat: 'aucune' },
  };
  let demarrage = false;
  const norm = (t: string) => t.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        switch (cmd) {
          case 'carnet_index':
            return { racine: reglages.dossier, reunions, conversations, rappels };
          case 'carnet_reunion':
            if (args.nom !== lue.resume!.nom) throw 'réunion introuvable';
            return lue;
          case 'carnet_conversation': {
            const c = conversations.find((x) => x.id === args.id);
            if (!c) throw 'conversation introuvable';
            return { resume: c, chemin: `/home/test/Kobi/conversations/${c.id}.jsonl`, tours: tours[c.id] };
          }
          case 'carnet_chercher': {
            const q = norm(String(args.requete).trim());
            const out: unknown[] = [];
            if (reunions.length) {
              for (const s of lue.segments)
                if (norm(s.text).includes(q)) out.push({ type: 'reunion', nom: lue.resume!.nom, titre: 'Point budget', partie: 'transcription', start: s.start, extrait: s.text });
              for (const l of lue.notes.split('\n'))
                if (norm(l).includes(q)) out.push({ type: 'reunion', nom: lue.resume!.nom, titre: 'Point budget', partie: 'notes', start: null, extrait: l });
            }
            for (const c of conversations)
              for (const t of tours[c.id] as Array<{ at_ms: number; texte: string }>)
                if (norm(t.texte).includes(q)) out.push({ type: 'conversation', id: c.id, at_ms: t.at_ms, extrait: t.texte });
            return out;
          }
          case 'carnet_markdown':
            return `# ${args.id}\n`;
          case 'carnet_exporter':
            return `/home/test/Documents/${String(args.id).replace('/', '-')}.md`;
          case 'carnet_ouvrir_dossier':
          case 'carnet_ouvrir_export':
            return null;
          case 'carnet_supprimer':
            if (args.genre === 'reunion') reunions.splice(0, 1);
            else conversations.splice(conversations.findIndex((c) => c.id === args.id), 1);
            return null;
          case 'carnet_renommer': {
            // Titre manuel (KOB-223) : le dossier d'une réunion suit son titre.
            const titre = String(args.titre);
            if (args.genre === 'reunion') {
              const r = reunions.find((x) => x.nom === args.id)!;
              r.titre = titre;
              r.nom = `2026-10-18-${norm(titre).replace(/[^a-z0-9]+/g, '-')}`;
              return r.nom;
            }
            (conversations.find((c) => c.id === args.id) as { titre?: string }).titre = titre;
            return args.id;
          }
          case 'carnet_rappel_fait':
            rappels.find((r) => r.id === args.id)!.fait = true;
            return true;
          case 'reglages_stockage':
            // Sans destination de publication (KOB-75).
            return w.__sansPublication ? { ...reglages, publication_reunions: { etat: 'aucune' } } : reglages;
          case 'carnet_publier':
            return 'Publie le compte rendu « Point budget » dans linear.';
          case 'choisir_dossier':
            return '/home/test/Ailleurs';
          case 'demarrage_systeme':
            return demarrage;
          case 'regler_demarrage_systeme':
            demarrage = Boolean(args.actif);
            return demarrage;
          case 'regler_stockage': {
            const c = args.changement as Record<string, unknown>;
            if (c.dossier !== undefined) reglages = { ...reglages, dossier: c.dossier || '/home/test/Kobi', par_defaut: !c.dossier };
            for (const k of ['retention_reunions', 'retention_conversations']) if (c[k] !== undefined) reglages = { ...reglages, [k]: c[k] };
            return reglages;
          }
          default:
            throw new Error(`commande inconnue ${cmd}`);
        }
      },
    },
    event: { listen: async () => () => {} },
  };
  // Le presse-papiers n'est pas permis en headless : on le remplace.
  Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (t: string) => void (w.__copie = t) } });
}

async function ouvrirCarnet(page: Page): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.addInitScript(fakeTauri);
  await page.setViewportSize({ width: 1120, height: 740 });
  await page.goto('/app.html#notes');
  await page.evaluate(() => document.fonts.ready);
  await expect(page.locator('.element')).toHaveCount(3);
  return errors;
}

const appels = (page: Page) => page.evaluate(() => (window as unknown as { __appels: Array<[string, unknown]> }).__appels);

test('liste réunions et conversations par jour, rappels à venir en tête', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await expect(page.locator('.jour-titre').first()).toHaveText('À venir');
  await expect(page.locator('.rappel')).toContainText('Appeler Léa');
  await expect(page.locator('.jour .jour-titre')).toHaveText(['Aujourd’hui']);
  // Plus récent d'abord : les deux conversations, puis la réunion.
  await expect(page.locator('.element .element-titre')).toHaveText(['Où est géré le refresh token ?', 'Quelle heure est-il à Tokyo ?', 'Point budget']);
  await expect(page.locator('.racine')).toHaveText('/home/test/Kobi');

  await page.getByRole('button', { name: 'Réunions', exact: true }).click();
  await expect(page.locator('.element')).toHaveCount(1);
  await page.getByRole('button', { name: 'Conversations', exact: true }).click();
  await expect(page.locator('.element')).toHaveCount(2);
  await expectNoErrors(errors);
});

test('vue d’une réunion : compte rendu, transcription horodatée, notes', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  await expect(page.locator('.element[data-genre="reunion"]')).toHaveAttribute('aria-current', 'true');
  // Le titre se pose après le brouillage.
  await expect(page.locator('.feuille .titre')).toHaveText('Point budget');
  await expect(page.locator('.surtitre')).toContainText('Réunion');
  await expect(page.locator('.surtitre')).toContainText('42 min');
  await expect(page.locator('[data-section="resume"]')).toContainText('Budget validé pour le prototype.');
  await expect(page.locator('[data-section="decisions"] strong')).toHaveText('fusée');
  await expect(page.locator('[data-section="actions"] li.tache')).toHaveCount(2);
  await expect(page.locator('[data-section="questions"]')).toHaveCount(0);

  await page.getByRole('tab', { name: /Transcription/ }).click();
  await expect(page.getByRole('tab', { name: /Transcription/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.parole')).toHaveCount(3);
  await expect(page.locator('.parole .temps')).toHaveText(['00:00', '00:05', '01:15']);
  await expect(page.locator('.parole .qui')).toHaveText(['Moi', 'Eux', 'Moi']);

  // Flèches du clavier entre onglets.
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: 'Notes' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.notes li')).toHaveText(['Budget : 12 k€', 'Fusée prête jeudi']);
  await expectNoErrors(errors);
});

test('vue d’une conversation : échanges Moi / Kobi', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="conversation"]').last().click();
  await expect(page.locator('.feuille .titre')).toHaveText('Quelle heure est-il à Tokyo ?');
  await expect(page.locator('.tour')).toHaveCount(2);
  await expect(page.locator('.tour[data-qui="moi"] .tour-tete')).toContainText('à la voix');
  await expect(page.locator('.tour[data-qui="kobi"] strong')).toHaveText('23 h');
  await expectNoErrors(errors);
});

test('la recherche retrouve un mot sans casse ni accents et mène au passage', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.getByRole('searchbox').fill('FUSEE');
  await expect(page.locator('.resultat')).toHaveCount(1);
  await expect(page.locator('.compte')).toHaveText('2 passages · 1 élément');
  await expect(page.locator('.extrait mark').first()).toHaveText('fusée');
  await expect(page.locator('.rappels')).toBeHidden();
  // Le passage de la transcription ouvre l'onglet et le met en évidence.
  await page.locator('.extrait', { hasText: '00:05' }).click();
  await expect(page.getByRole('tab', { name: /Transcription/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.parole.cible')).toHaveAttribute('data-start', '5.5');
  await expect(page.locator('.parole.cible mark')).toHaveText('fusée');

  // Un mot d'une conversation.
  await page.getByRole('searchbox').fill('tokyo');
  await expect(page.locator('.resultat')).toHaveCount(1);
  // Le compte est le même qu'avant : on attend les résultats de « tokyo ».
  await expect(page.locator('.extrait mark').first()).toHaveText('Tokyo');
  await page.locator('.extrait').first().click();
  await expect(page.locator('.feuille[data-genre="conversation"]')).toBeVisible();
  await expect(page.locator('.tour.cible mark')).toHaveText('Tokyo');
  // L'extrait de la réponse de Kobi est montré sans ses marques markdown.
  await expect(page.locator('.extrait-texte').last()).toHaveText('Il est 23 h à Tokyo.');

  // Rien trouvé, puis Échap revient à la liste.
  await page.getByRole('searchbox').fill('zeppelin');
  await expect(page.locator('.vide')).toHaveText('Aucune trace de « zeppelin ».');
  await page.getByRole('searchbox').press('Escape');
  await expect(page.locator('.element')).toHaveCount(3);
  await expectNoErrors(errors);
});

const copie = (page: Page) => page.evaluate(() => (window as unknown as { __copie: string }).__copie);
const nbCar = (t: string) => `${new Intl.NumberFormat('fr-FR').format([...t].length)} caractères`;

const CR_MD = '## Résumé\n\nBudget validé pour le prototype.\n\n## Décisions\n\n- On garde la **fusée** v2\n\n## Actions\n\n- [ ] Envoyer le devis\n- [x] Réserver la salle';
const TRANSCRIPTION_MD = '- `00:00` **Moi** : Bonjour à tous, on parle du budget.\n- `00:05` **Eux** : Le prototype de la fusée vole jeudi.\n- `01:15` **Moi** : Parfait, on valide.';
const NOTES_MD = '- Budget : 12 k€\n- Fusée prête jeudi';

test('copier, exporter, ouvrir le dossier', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  await page.getByRole('button', { name: 'Copier', exact: true }).click();
  await page.getByRole('menuitem', { name: /Copier tout/ }).click();
  await expect(page.locator('.notif .notif-texte')).toHaveText(`Réunion copiée en entier · ${nbCar('# 2026-10-18_point-budget\n')} · markdown`);
  expect(await copie(page)).toBe('# 2026-10-18_point-budget\n');
  await page.getByRole('button', { name: 'Exporter…' }).click();
  await expect(page.locator('.notif').last()).toContainText('Exporté en markdown : 2026-10-18_point-budget.md');
  await expect(page.locator('.notif').last().locator('.notif-apercu')).toHaveText('titre, compte rendu et transcription\ndans /home/test/Documents/');
  // Un seul message à la fois : « Exporté » remplace « Copié » (KOB-215).
  await expect(page.locator('.notif:not(.sort)')).toHaveCount(1);
  await expect(page.locator('.notif', { hasText: 'Réunion copiée' })).toHaveCount(0);
  // « Ouvrir » ouvre le fichier exporté ; la coque sait lequel, la fenêtre ne donne aucun chemin (KOB-224).
  await page.locator('.notif:not(.sort)').getByRole('button', { name: 'Ouvrir' }).click();
  expect(await appels(page)).toContainEqual(['carnet_ouvrir_export', {}]);
  await expect(page.locator('.notif:not(.sort)')).toHaveCount(0);
  await page.getByRole('button', { name: 'Ouvrir le dossier' }).click();
  expect(await appels(page)).toContainEqual(['carnet_ouvrir_dossier', { genre: 'reunion', id: '2026-10-18_point-budget' }]);
  await expectNoErrors(errors);
});

test('« Publier » (KOB-75) : le compte rendu part vers la destination réglée, après la carte de Kobi', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const publier = page.locator('.actions [data-action="publier"]');
  await expect(publier).toHaveText('Publier');
  await expect(publier).toHaveAttribute('data-reglee', 'true');
  await expect(publier).toHaveAttribute('title', /linear \(Projet Kobi\)/);
  await publier.click();
  expect(await appels(page)).toContainEqual(['carnet_publier', { nom: '2026-10-18_point-budget' }]);
  await expect(page.locator('.notif').last()).toContainText('Publie le compte rendu « Point budget » dans linear.');
  await expect(page.locator('.notif').last()).toContainText('confirmez sur sa carte');
  // Pas de bouton pour une conversation.
  await page.locator('.element[data-genre="conversation"]').first().click();
  await expect(page.locator('.actions [data-action="publier"]')).toHaveCount(0);
  await expectNoErrors(errors);
});

test('« Publier » sans destination (KOB-75) : rien ne part, le Carnet propose de la régler', async ({ page }) => {
  await page.addInitScript(() => ((window as unknown as Record<string, unknown>).__sansPublication = true));
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const publier = page.locator('.actions [data-action="publier"]');
  await expect(publier).toHaveAttribute('data-reglee', 'false');
  await publier.click();
  await expect(page.locator('.notif').last()).toContainText('Choisissez d’abord où publier');
  expect((await appels(page)).some(([c]) => c === 'carnet_publier')).toBe(false);
  await page.locator('.notif').last().getByRole('button', { name: 'Régler' }).click();
  await expect(page).toHaveURL(/#reglages\/stockage$/);
  await expectNoErrors(errors);
});

test('« Publier » relit la destination réglée après coup, sans rouvrir la réunion (KOB-264)', async ({ page }) => {
  await page.addInitScript(() => ((window as unknown as Record<string, unknown>).__sansPublication = true));
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const publier = page.locator('.actions [data-action="publier"]');
  await expect(publier).toHaveAttribute('data-reglee', 'false');
  // Réglée ensuite (Réglages › Stockage, ou par Claude) : la vue n'est pas redessinée.
  await page.evaluate(() => ((window as unknown as Record<string, unknown>).__sansPublication = false));
  await publier.hover();
  await expect(publier).toHaveAttribute('data-reglee', 'true');
  await expect(publier).toHaveAttribute('title', /linear \(Projet Kobi\)/);
  await publier.click();
  expect(await appels(page)).toContainEqual(['carnet_publier', { nom: '2026-10-18_point-budget' }]);
  // Retirée ensuite : le clic propose de nouveau de la régler, rien ne part.
  await page.evaluate(() => ((window as unknown as Record<string, unknown>).__sansPublication = true));
  const avant = (await appels(page)).filter(([c]) => c === 'carnet_publier').length;
  await page.mouse.move(0, 0);
  await publier.click();
  await expect(page.locator('.notif').last()).toContainText('Choisissez d’abord où publier');
  expect((await appels(page)).filter(([c]) => c === 'carnet_publier').length).toBe(avant);
  await expectNoErrors(errors);
});

test('« Copier » dit ce qu’on copie : compte rendu, transcription, notes, tout, avec leur taille ; chacun met exactement son contenu (KOB-224)', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const bouton = page.getByRole('button', { name: 'Copier', exact: true });
  await bouton.click();
  await expect(bouton).toHaveAttribute('aria-expanded', 'true');
  const menu = page.getByRole('menu', { name: 'Que copier ?' });
  await expect(menu.getByRole('menuitem')).toHaveText([
    `Copier le compte rendu${nbCar(CR_MD)}`,
    'Copier la transcription3 lignes',
    `Copier les notes${nbCar(NOTES_MD)}`,
    'Copier toutmarkdown : titre, compte rendu et transcription',
  ]);
  await menu.evaluate((m) => Promise.all(m.getAnimations({ subtree: true }).map((a) => a.finished)));
  await page.screenshot({ path: test.info().outputPath('menu-copier.png') });
  const cas: Array<[RegExp, string, string]> = [
    [/^Copier le compte rendu/, CR_MD, 'Compte rendu copié'],
    [/^Copier la transcription/, TRANSCRIPTION_MD, 'Transcription copiée'],
    [/^Copier les notes/, NOTES_MD, 'Notes copiées'],
  ];
  for (const [nom, attendu, debut] of cas) {
    if (await menu.isHidden()) await bouton.click();
    await menu.getByRole('menuitem', { name: nom }).click();
    await expect(menu).toBeHidden();
    expect(await copie(page)).toBe(attendu);
    const notif = page.locator('.notif:not(.sort)');
    await expect(notif.locator('.notif-texte')).toHaveText(`${debut} · ${nbCar(attendu)} · markdown`);
    // Aperçu : les deux premières lignes non vides.
    await expect(notif.locator('.notif-apercu')).toHaveText(attendu.split('\n').filter((l) => l.trim()).slice(0, 2).join('\n'));
  }
  await page.screenshot({ path: test.info().outputPath('retour-copie.png') });
  await expectNoErrors(errors);
});

test('format « texte brut », retenu : sans dièses ni astérisques, puces en « • » (KOB-224)', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  await page.getByRole('button', { name: 'Copier', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'texte brut' }).click();
  await expect(page.getByRole('menuitemradio', { name: 'texte brut' })).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('menuitem', { name: /^Copier le compte rendu/ }).click();
  expect(await copie(page)).toBe('Résumé\n\nBudget validé pour le prototype.\n\nDécisions\n\n• On garde la fusée v2\n\nActions\n\n• Envoyer le devis\n• Réserver la salle');
  await expect(page.locator('.notif:not(.sort) .notif-texte')).toContainText('· texte brut');
  // Retenu : rouvert, le menu montre toujours « texte brut ».
  await page.reload();
  await page.locator('.element[data-genre="reunion"]').click();
  await page.getByRole('button', { name: 'Copier', exact: true }).click();
  await expect(page.getByRole('menuitemradio', { name: 'texte brut' })).toHaveAttribute('aria-checked', 'true');
  await expectNoErrors(errors);
});

test('copie au survol : chaque partie, décision, action, note et ligne de transcription, libellée (KOB-224)', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const decision = page.locator('[data-section="decisions"] li').first();
  const bouton = decision.getByRole('button', { name: 'Copier cette décision' });
  await expect(bouton).toHaveCSS('opacity', '0');
  await decision.hover();
  await expect(bouton).toHaveCSS('opacity', '1');
  await bouton.hover();
  await expect(bouton.locator('.copie-libelle')).toHaveAttribute('data-libelle', 'Copier cette décision');
  await expect(bouton).toHaveCSS('gap', '6px');
  await expect(bouton.locator('.copie-libelle')).toHaveCSS('max-width', '220px');
  await page.waitForTimeout(300); // fin de la transition de largeur, pour la capture
  await page.screenshot({ path: test.info().outputPath('survol-decision.png') });
  await bouton.click();
  expect(await copie(page)).toBe('- On garde la **fusée** v2');
  await expect(page.locator('.notif:not(.sort) .notif-texte')).toHaveText(`Décision copiée · ${nbCar('- On garde la **fusée** v2')} · markdown`);

  await page.locator('[data-section="actions"] li').nth(1).hover();
  await page.locator('[data-section="actions"] li').nth(1).getByRole('button', { name: 'Copier cette action' }).click();
  expect(await copie(page)).toBe('- [x] Réserver la salle');
  await page.locator('[data-section="resume"]').hover();
  await page.getByRole('button', { name: 'Copier le résumé' }).click();
  expect(await copie(page)).toBe('Budget validé pour le prototype.');
  await page.locator('[data-section="decisions"]').getByRole('button', { name: 'Copier les décisions' }).focus();
  await page.keyboard.press('Enter');
  expect(await copie(page)).toBe('- On garde la **fusée** v2');

  await page.getByRole('tab', { name: /Transcription/ }).click();
  const ligne = page.locator('.parole').nth(1);
  await ligne.hover();
  await ligne.getByRole('button', { name: 'Copier cette ligne' }).click();
  expect(await copie(page)).toBe('- `00:05` **Eux** : Le prototype de la fusée vole jeudi.');

  await page.getByRole('tab', { name: 'Notes' }).click();
  await page.locator('.notes li').nth(1).hover();
  await page.locator('.notes li').nth(1).getByRole('button', { name: 'Copier cette note' }).click();
  expect(await copie(page)).toBe('- Fusée prête jeudi');
  await expect(page.locator('.notif:not(.sort)')).toHaveCount(1);
  await expectNoErrors(errors);
});

test('menu « Copier » au clavier : flèches, Entrée, Échap rend le focus ; les parties vides sont grisées (KOB-224)', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const bouton = page.getByRole('button', { name: 'Copier', exact: true });
  await bouton.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('menuitem', { name: /^Copier le compte rendu/ })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitem', { name: /^Copier la transcription/ })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menu')).toBeHidden();
  await expect(bouton).toBeFocused();
  await page.keyboard.press('Enter');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  expect(await copie(page)).toBe(TRANSCRIPTION_MD);
  // Clic à côté : le menu se ferme.
  await bouton.click();
  await page.locator('.titre').click();
  await expect(page.getByRole('menu')).toBeHidden();
  await expectNoErrors(errors);
});

test('supprimer demande confirmation dans la fenêtre, jamais par confirm()', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  page.on('dialog', () => errors.push('dialogue natif ouvert'));
  await page.locator('.element[data-genre="reunion"]').click();
  await page.getByRole('button', { name: 'Supprimer' }).click();
  const confirmation = page.getByRole('alertdialog');
  await expect(confirmation).toContainText('Supprimer « Point budget » ?');
  await expect(page.getByRole('button', { name: 'Annuler' })).toBeFocused();
  // Échap annule : rien n'est supprimé.
  await page.keyboard.press('Escape');
  await expect(confirmation).toHaveCount(0);
  expect((await appels(page)).some(([c]) => c === 'carnet_supprimer')).toBe(false);

  await page.getByRole('button', { name: 'Supprimer' }).click();
  await page.getByRole('button', { name: 'Supprimer définitivement' }).click();
  await expect(page.locator('.notif')).toHaveText('Réunion supprimée.');
  await expect(page.locator('.element')).toHaveCount(2);
  await expect(page.locator('.vide-titre')).toBeVisible();
  await expectNoErrors(errors);
});

test('un rappel se coche comme fait', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.getByRole('button', { name: 'Marquer « Appeler Léa » comme fait' }).click();
  await expect(page.locator('.notif')).toHaveText('Rappel fait.');
  await expect(page.locator('.rappels')).toBeHidden();
  expect(await appels(page)).toContainEqual(['carnet_rappel_fait', { id: 'r1' }]);
  await expectNoErrors(errors);
});

test('panneau Stockage : dossier, sélecteur, conservation, publication en lecture seule', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  // Le bouton du pied du Carnet mène à Réglages › Stockage, dans l'app.
  await page.getByRole('button', { name: 'Stockage' }).click();
  await expect(page).toHaveURL(/#reglages\/stockage$/);
  await expect(page.locator('.vue-reglages .titre')).toHaveText('Stockage');
  await expect(page.locator('.chemin')).toHaveText('/home/test/Kobi');
  await expect(page.locator('.dossier .puce')).toHaveText('par défaut');
  // Conservation illimitée par défaut.
  const reunions = page.getByRole('radiogroup', { name: 'Garder les réunions' });
  await expect(reunions.getByRole('radio', { name: 'Toujours' })).toHaveAttribute('aria-checked', 'true');
  await reunions.getByRole('radio', { name: '90 jours' }).click();
  await expect(page.locator('.notif')).toHaveText('Conservation des réunions enregistrée.');
  expect(await appels(page)).toContainEqual(['regler_stockage', { changement: { retention_reunions: 90 } }]);
  await expect(reunions.getByRole('radio', { name: '90 jours' })).toHaveAttribute('aria-checked', 'true');

  // Publication : montrée, pas modifiable ici ; les réunions ont le bouton « Publier » du Carnet (KOB-75).
  await expect(page.locator('.publication[data-etat="reglee"]')).toContainText('linear → Projet Kobi');
  await expect(page.locator('.publication[data-etat="reglee"]')).toContainText('Bouton « Publier » du Carnet');
  await expect(page.locator('.publications input')).toHaveCount(0);

  // Sélecteur natif, puis le dossier change et l'index suit.
  await page.getByRole('button', { name: 'Choisir un dossier…' }).click();
  await expect(page.locator('.chemin')).toHaveText('/home/test/Ailleurs');
  expect(await appels(page)).toContainEqual(['regler_stockage', { changement: { dossier: '/home/test/Ailleurs' } }]);
  // De retour dans les notes, l'index suit le nouveau dossier.
  await page.getByRole('button', { name: 'Notes et réunions' }).click();
  await expect(page.locator('.racine')).toHaveText('/home/test/Ailleurs');
  await page.getByRole('button', { name: 'Réglages' }).click();
  await page.getByRole('button', { name: 'Revenir au dossier par défaut' }).click();
  await expect(page.locator('.chemin')).toHaveText('/home/test/Kobi');
  await expectNoErrors(errors);
});

test('la section Réglages de l’app monte les mêmes panneaux', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.goto('/app.html#reglages/stockage');
  await expect(page.locator('.menu-entree[aria-current="true"]')).toContainText('Stockage');
  await expect(page.locator('.titre')).toHaveText('Stockage');
  await expect(page.locator('.chemin')).toHaveText('/home/test/Kobi');
  await expectNoErrors(errors);
});

test('panneau Démarrage des Réglages : interrupteur branché sur le démarrage avec le système', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.goto('/app.html#reglages/demarrage');
  await expect(page.locator('.titre')).toHaveText('Démarrage');
  const interrupteur = page.getByRole('switch', { name: 'Lancer Kobi à l’ouverture de session' });
  await expect(interrupteur).toHaveAttribute('aria-checked', 'false');
  await interrupteur.click();
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'true');
  await expect(page.locator('.notif')).toHaveText('Kobi démarrera avec ta session.');
  expect(await appels(page)).toContainEqual(['regler_demarrage_systeme', { actif: true }]);
  await expectNoErrors(errors);
});

test('animations réduites : titres posés sans brouillage, rien ne bouge', async ({ page }) => {
  // Réglage « Animations : réduites » de Kobi : la classe sur l'élément racine.
  await page.addInitScript(() => {
    const poser = () => document.documentElement?.classList.add('reduced-motion');
    poser();
    document.addEventListener('readystatechange', poser);
  });
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  // Posé tout de suite, sans passer par les symboles.
  expect(await page.locator('.feuille .titre').textContent()).toBe('Point budget');
  const animees = await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length);
  expect(animees).toBe(0);
  await expectNoErrors(errors);
});

test('hors de Kobi, le Carnet le dit au lieu de planter', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  // Horloge arrêtée : le message s'efface seul après quelques secondes
  // (KOB-215), délai qu'un PC chargé dépasse avant que le test ne le lise.
  await page.clock.install();
  await page.clock.pauseAt(Date.now() + 1000);
  await page.goto('/app.html#notes');
  await expect(page.locator('.notif', { hasText: 'Ouvrez le Carnet depuis Kobi.' })).toBeVisible();
  await expectNoErrors(errors);
});

test('KOB-223 : le titre d’une réunion s’édite sur place (en-tête et liste), Entrée valide, Échap annule', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const titre = page.locator('.entete h1.titre');
  await expect(titre).toHaveText('Point budget');

  // Échap annule : rien n'est envoyé.
  await titre.click();
  const champ = page.locator('.entete .titre-champ');
  await expect(champ).toBeFocused();
  await expect(champ).toHaveValue('Point budget');
  expect(await champ.evaluate((e: HTMLInputElement) => [e.selectionStart, e.selectionEnd])).toEqual([0, 'Point budget'.length]);
  await champ.fill('Autre chose');
  await champ.press('Escape');
  await expect(titre).toHaveText('Point budget');
  expect((await appels(page)).some(([c]) => c === 'carnet_renommer')).toBe(false);

  // Entrée valide : retour visuel immédiat, puis la liste et la vue suivent le nouveau nom.
  await page.locator('.entete .crayon').click();
  await champ.fill('Budget du prototype');
  await champ.press('Enter');
  await expect(titre).toHaveText('Budget du prototype');
  await expect(page.locator('.element[data-genre="reunion"] .element-titre')).toHaveText('Budget du prototype');
  const renommer = (await appels(page)).filter(([c]) => c === 'carnet_renommer');
  expect(renommer).toEqual([['carnet_renommer', { genre: 'reunion', id: '2026-10-18_point-budget', titre: 'Budget du prototype' }]]);
  await expect(page.locator('.element[data-genre="reunion"]')).toHaveAttribute('data-id', '2026-10-18-budget-du-prototype');

  // Depuis la liste : crayon au survol de la ligne.
  const ligne = page.locator('.element-ligne[data-genre="conversation"]').first();
  await ligne.hover();
  await ligne.locator('.crayon').click();
  const champListe = ligne.locator('.titre-champ');
  await expect(champListe).toBeFocused();
  await champListe.fill('Refresh token');
  await champListe.press('Enter');
  await expect(ligne.locator('.element-titre')).toHaveText('Refresh token');
  await expectNoErrors(errors);
});

test('réunion en cours : la durée suit l’horloge dans la liste et dans l’en-tête, sans redessiner la liste (KOB-251)', async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => ((window as unknown as { __enCours: boolean }).__enCours = true));
  const errors = await ouvrirCarnet(page);
  const ligne = page.locator('.element[data-genre="reunion"] .element-meta');
  await expect(ligne).toContainText('12 min');
  await page.locator('.element[data-genre="reunion"]').click();
  const surtitre = page.locator('.feuille[data-genre="reunion"] .surtitre');
  await expect(surtitre).toContainText('12 min');
  // La liste n'est pas redessinée : la même ligne garde son nœud.
  await page.evaluate(() => ((window as unknown as { __ligne: Element | null }).__ligne = document.querySelector('.element[data-genre="reunion"]')));
  await page.clock.runFor(2 * 60_000);
  await expect(ligne).toContainText('14 min');
  await expect(surtitre).toContainText('14 min');
  await expect(surtitre.locator('.rec')).toHaveText('REC');
  expect(await page.evaluate(() => (window as unknown as { __ligne: Element | null }).__ligne === document.querySelector('.element[data-genre="reunion"]'))).toBe(true);
  await expectNoErrors(errors);
});

test('KOB-246 (4.4) : le soulignement épouse l’onglet choisi une fois sa graisse posée ; survoler un onglet ne pousse pas ses voisins', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  const tab = (nom: RegExp) => page.getByRole('tab', { name: nom });
  const gauches = () => page.locator('.onglet').evaluateAll((bs) => bs.map((b) => Math.round((b as HTMLElement).offsetLeft)));
  const avant = await gauches();
  // Onglet du milieu, pas encore choisi : sa graisse change au survol.
  await tab(/Transcription/).hover();
  await page.waitForTimeout(450);
  expect(await gauches()).toEqual(avant);
  await tab(/Transcription/).click();
  // Après la transition de graisse (0,3 s) et le ressort (0,45 s).
  await page.waitForTimeout(800);
  const { indicateur, onglet } = await page.evaluate(() => {
    const i = document.querySelector<HTMLElement>('.onglet-indicateur')!.getBoundingClientRect();
    const o = document.querySelector<HTMLElement>('.onglet[aria-selected="true"]')!.getBoundingClientRect();
    return { indicateur: { x: i.x, l: i.width }, onglet: { x: o.x, l: o.width } };
  });
  expect(Math.abs(indicateur.x - onglet.x)).toBeLessThanOrEqual(1);
  expect(Math.abs(indicateur.l - onglet.l)).toBeLessThanOrEqual(1);
  expect(await gauches()).toEqual(avant);
  await expectNoErrors(errors);
});
