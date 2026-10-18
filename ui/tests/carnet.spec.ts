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
    publication_reunions: { etat: 'lecture-seule', mcp: 'linear', destination: 'Projet Kobi' },
    publication_conversations: { etat: 'aucune' },
  };
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
            return null;
          case 'carnet_supprimer':
            if (args.genre === 'reunion') reunions.splice(0, 1);
            else conversations.splice(conversations.findIndex((c) => c.id === args.id), 1);
            return null;
          case 'carnet_rappel_fait':
            rappels.find((r) => r.id === args.id)!.fait = true;
            return true;
          case 'reglages_stockage':
            return reglages;
          case 'choisir_dossier':
            return '/home/test/Ailleurs';
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
  await page.goto('/carnet.html');
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

test('copier, exporter, ouvrir le dossier', async ({ page }) => {
  const errors = await ouvrirCarnet(page);
  await page.locator('.element[data-genre="reunion"]').click();
  await page.getByRole('button', { name: 'Copier' }).click();
  await expect(page.locator('.notif')).toHaveText('Copié en markdown.');
  expect(await page.evaluate(() => (window as unknown as { __copie: string }).__copie)).toBe('# 2026-10-18_point-budget\n');
  await page.getByRole('button', { name: 'Exporter' }).click();
  await expect(page.locator('.notif').last()).toContainText('Exporté : /home/test/Documents/');
  await page.getByRole('button', { name: 'Ouvrir le dossier' }).click();
  expect(await appels(page)).toContainEqual(['carnet_ouvrir_dossier', { genre: 'reunion', id: '2026-10-18_point-budget' }]);
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
  await page.getByRole('button', { name: 'Stockage' }).click();
  await expect(page.locator('.feuille .titre')).toHaveText('Stockage');
  await expect(page.locator('.chemin')).toHaveText('/home/test/Kobi');
  await expect(page.locator('.dossier .puce')).toHaveText('par défaut');
  // Conservation illimitée par défaut.
  const reunions = page.getByRole('radiogroup', { name: 'Garder les réunions' });
  await expect(reunions.getByRole('radio', { name: 'Toujours' })).toHaveAttribute('aria-checked', 'true');
  await reunions.getByRole('radio', { name: '90 jours' }).click();
  await expect(page.locator('.notif')).toHaveText('Conservation des réunions enregistrée.');
  expect(await appels(page)).toContainEqual(['regler_stockage', { changement: { retention_reunions: 90 } }]);
  await expect(reunions.getByRole('radio', { name: '90 jours' })).toHaveAttribute('aria-checked', 'true');

  // Publication : montrée, pas modifiable.
  await expect(page.locator('.publication[data-etat="lecture-seule"]')).toContainText('linear → Projet Kobi');
  await expect(page.locator('.publication[data-etat="lecture-seule"]')).toContainText('rien n’est envoyé');
  await expect(page.locator('.publications input')).toHaveCount(0);

  // Sélecteur natif, puis le dossier change et l'index suit.
  await page.getByRole('button', { name: 'Choisir un dossier…' }).click();
  await expect(page.locator('.chemin')).toHaveText('/home/test/Ailleurs');
  await expect(page.locator('.racine')).toHaveText('/home/test/Ailleurs');
  expect(await appels(page)).toContainEqual(['regler_stockage', { changement: { dossier: '/home/test/Ailleurs' } }]);
  await page.getByRole('button', { name: 'Revenir au dossier par défaut' }).click();
  await expect(page.locator('.chemin')).toHaveText('/home/test/Kobi');
  await expectNoErrors(errors);
});

test('la fenêtre Réglages monte les mêmes panneaux', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.goto('/reglages.html?panneau=stockage');
  await expect(page.locator('.menu-entree[aria-current="true"]')).toContainText('Stockage');
  await expect(page.locator('.titre')).toHaveText('Stockage');
  await expect(page.locator('.chemin')).toHaveText('/home/test/Kobi');
  await expectNoErrors(errors);
});

test('animations réduites : titres posés sans brouillage, rien ne bouge', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
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
  await page.goto('/carnet.html');
  await expect(page.locator('.notif')).toHaveText('Ouvrez le Carnet depuis Kobi.');
  await expectNoErrors(errors);
});
