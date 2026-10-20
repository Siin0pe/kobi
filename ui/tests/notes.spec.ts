// Notes en direct (KOB-73), section `direct` de l'app Kobi (KOB-159), avec
// une fausse coque Tauri. La lecture réelle du `notes.md` est testée côté
// Rust (crates/kobi-app/src/notes.rs).
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

function fakeTauri(): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  w.__notes = null;
  w.__rec ??= { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null };
  w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  w.__TAURI__ = {
    window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        if (cmd === 'notes_en_cours') return w.__notes;
        if (cmd === 'suivre_transcription') return null;
        if (cmd === 'basculer_notes') return true;
        if (cmd === 'etat_enregistrement') return w.__rec;
        if (cmd === 'etat_session') return { etat: 'reunion', activite: null, mascotte: 'write', transcrit: true };
        if (cmd === 'carnet_index') return { racine: '/home/test/Kobi', reunions: [], conversations: [], rappels: [] };
        throw new Error(`commande inconnue ${cmd}`);
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

const NOTES = {
  reunion: '2026-10-18-point-hebdo',
  titre: 'Point hebdo',
  debut_ms: Date.now() - 12 * 60_000,
  decisions: '- Budget validé\n- Séminaire à Annecy',
  actions: '- [ ] Sophie : devis',
  questions: '',
  autres: [],
  derniere_note: 'Sophie : devis',
  lignes: 3,
  ecrit_ms: Date.now(),
};

async function ouvrirNotes(page: Page, notes: unknown): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.addInitScript(fakeTauri);
  await page.addInitScript((n) => ((window as unknown as Record<string, unknown>).__notes = n), notes);
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto('/app.html#direct');
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

const appels = (page: Page) => page.evaluate(() => (window as unknown as { __appels: Array<[string, unknown]> }).__appels);

test('décisions, actions, questions et dernière note mise en valeur', async ({ page }) => {
  const errors = await ouvrirNotes(page, NOTES);
  await expect(page.locator('.vue-direct .titre')).toHaveText('Point hebdo');
  await expect(page.locator('.vue-direct .duree')).toContainText(/ · 12:0\d/);
  await expect(page.locator('.vue-direct .derniere-texte')).toHaveText('Sophie : devis');
  await expect(page.locator('.vue-direct [data-section="decisions"] li')).toHaveText(['Budget validé', 'Séminaire à Annecy']);
  await expect(page.locator('.vue-direct [data-section="decisions"] .nombre')).toHaveText('2');
  await expect(page.locator('.vue-direct [data-section="actions"] li.tache')).toHaveCount(1);
  await expect(page.locator('.vue-direct [data-section="questions"] .vide')).toHaveText('Aucune question ouverte.');
  await expectNoErrors(errors);
});

test('transcription brute (KOB-50) : masquée par défaut, Ctrl+Alt+T l’ouvre ; partiels en gris remplacés, finaux en clair, piste moi / eux', async ({ page }) => {
  const errors = await ouvrirNotes(page, NOTES);
  const panneau = page.locator('.vue-direct .panneau-brut');
  const suivis = async () => (await appels(page)).filter(([c]) => c === 'suivre_transcription').map(([, a]) => (a as { actif: boolean }).actif);
  await expect(page.locator('.vue-direct .derniere-texte')).toHaveText('Sophie : devis');
  await expect(panneau).toBeHidden();
  expect(await suivis()).not.toContain(true);
  await page.keyboard.press('Control+Alt+t');
  await expect(panneau).toBeVisible();
  await expect(panneau.locator('.brut-vide')).toHaveText('En attente de parole…');
  await expect.poll(suivis).toContain(true);
  const emettre = (source: string, texte: string, finale: boolean) =>
    page.evaluate(([source, texte, finale]) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('transcription_brute', { source, texte, finale }), [source, texte, finale] as const);
  await emettre('moi', 'On commence', false);
  await emettre('moi', 'On commence le point hebdo', false);
  await emettre('eux', 'Bonjour à', false);
  await emettre('moi', 'On commence le point hebdo.', true);
  await emettre('eux', 'Bonjour à tous, le budget', false);
  const lignes = panneau.locator('.brut-lignes li');
  await expect(lignes).toHaveCount(2);
  await expect(lignes.nth(0).locator('.brut-source')).toHaveText('moi');
  await expect(lignes.nth(0).locator('.brut-texte')).toHaveText('On commence le point hebdo.');
  await expect(lignes.nth(0)).not.toHaveClass(/partiel/);
  await expect(lignes.nth(1).locator('.brut-source')).toHaveText('eux');
  await expect(lignes.nth(1)).toHaveClass(/partiel/);
  await expect(panneau.locator('.brut-vide')).toBeHidden();
  // Gris : le partiel est plus pâle que le final.
  const couleur = (i: number) => lignes.nth(i).evaluate((n) => getComputedStyle(n).color);
  expect(await couleur(1)).not.toBe(await couleur(0));
  await emettre('eux', 'Bonjour à tous, le budget est tenu.', true);
  await emettre('moi', 'Merci', false);
  await expect(lignes).toHaveCount(3);
  await expect(lignes.nth(1)).not.toHaveClass(/partiel/);
  if (process.env.CAPTURE_KOB50) await page.screenshot({ path: process.env.CAPTURE_KOB50 });
  // Refermé : plus rien n'est demandé à la coque.
  await page.keyboard.press('Control+Alt+t');
  await expect(panneau).toBeHidden();
  await expect.poll(async () => (await suivis()).at(-1)).toBe(false);
  await expectNoErrors(errors);
});

test('une nouvelle note arrive : relue sur l’événement, mise en valeur, sections repliées gardées', async ({ page }) => {
  const errors = await ouvrirNotes(page, NOTES);
  await expect(page.locator('.vue-direct .derniere-texte')).toHaveText('Sophie : devis');
  // Section Décisions repliée par l'utilisateur.
  await page.locator('.vue-direct [data-section="decisions"] summary').click();
  await expect(page.locator('.vue-direct [data-section="decisions"]')).not.toHaveAttribute('open', '');
  await page.evaluate(() => {
    const w = window as unknown as { __notes: Record<string, unknown>; __emit: (e: string, p: unknown) => void };
    w.__notes = { ...w.__notes, questions: '- Date du séminaire ?', derniere_note: 'Date du séminaire ?', lignes: 4 };
    w.__emit('notes', '');
  });
  await expect(page.locator('.vue-direct .derniere-texte')).toHaveText('Date du séminaire ?');
  await expect(page.locator('.vue-direct .derniere')).toHaveClass(/arrive/);
  await expect(page.locator('.vue-direct [data-section="questions"] li')).toHaveText(['Date du séminaire ?']);
  await expect(page.locator('.vue-direct [data-section="questions"] li')).toHaveClass(/nouvelle/);
  // Seul le premier affichage entre en cascade : un rafraîchissement ne rejoue rien.
  await expect(page.locator('.vue-direct .sections')).not.toHaveClass(/premier/);
  // Les anciennes puces ne sont pas « nouvelles », la section repliée le reste.
  await expect(page.locator('.vue-direct [data-section="actions"] li.nouvelle')).toHaveCount(0);
  await expect(page.locator('.vue-direct [data-section="decisions"]')).not.toHaveAttribute('open', '');
  await expectNoErrors(errors);
});

test('pendant une réunion, le bandeau REC mène aux notes en direct depuis toutes les sections', async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as Record<string, unknown>).__rec = { actif: true, pause: false, debut_ms: Date.now() - 65_000, notes: 3, titre: 'Point hebdo' };
  });
  const errors = await ouvrirNotes(page, NOTES);
  await page.evaluate(() => window.kobiApp.aller('rappels'));
  const bandeau = page.locator('.bandeau-rec');
  await expect(bandeau).toBeVisible();
  await expect(bandeau).toContainText('REC');
  await expect(bandeau.locator('.rec-titre')).toHaveText('Point hebdo');
  await expect(bandeau.locator('.rec-duree')).toHaveText(/^1:0\d$/);
  await expect(bandeau.locator('.rec-etat')).toHaveText('3 notes');
  await expect(page.getByRole('button', { name: 'En direct', exact: true })).toBeVisible();
  await bandeau.click();
  await expect(page.locator('.vue:not([hidden])')).toHaveAttribute('data-section', 'direct');
  await expect(page).toHaveURL(/#direct$/);
  await expect(page.locator('.vue-direct .derniere-texte')).toHaveText('Sophie : devis');
  // Pause, puis fin de la réunion : le bandeau suit, puis s'efface.
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('enregistrement', { actif: true, pause: true, debut_ms: Date.now(), notes: 3, titre: 'Point hebdo' }));
  await expect(bandeau).toHaveAttribute('data-pause', 'true');
  await expect(bandeau.locator('.rec-etat')).toHaveText('en pause');
  await page.evaluate(() => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('enregistrement', { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null }));
  await expect(bandeau).toBeHidden();
  await page.evaluate(() => window.kobiApp.aller('notes'));
  await expect(page.getByRole('button', { name: 'En direct', exact: true })).toBeHidden();
  await expectNoErrors(errors);
});

test('avant la première note, puis hors réunion', async ({ page }) => {
  const errors = await ouvrirNotes(page, { ...NOTES, decisions: '', actions: '', derniere_note: null, lignes: 0 });
  await expect(page.locator('.vue-direct .derniere .vide')).toContainText('les premières notes arrivent');
  await page.evaluate(() => {
    const w = window as unknown as { __notes: unknown; __emit: (e: string, p: unknown) => void };
    w.__notes = null;
    w.__emit('enregistrement', { actif: false });
  });
  await expect(page.locator('.vue-direct .titre')).toHaveText('Pas de réunion');
  await expect(page.locator('.vue-direct .derniere .vide')).toContainText('pendant une réunion enregistrée');
  await expectNoErrors(errors);
});

test('depuis Kobi, un clic sur REC ou sur la fiche de notes ouvre le panneau', async ({ page }) => {
  await page.addInitScript(fakeTauri);
  await openUi(page, 'backdrop=1');
  // Réunion en cours, comme l'annonce la coque (événement `enregistrement`).
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now(), notes: 2, titre: 'Point hebdo', derniere_note: 'Budget validé' }));
  const rec = page.locator('.rec');
  await expect(rec).toBeVisible();
  await expect(rec).toHaveAttribute('role', 'button');
  await rec.click();
  await expect.poll(async () => (await appels(page)).filter(([c]) => c === 'basculer_notes').length).toBe(1);
  await rec.focus();
  await page.keyboard.press('Enter');
  await expect.poll(async () => (await appels(page)).filter(([c]) => c === 'basculer_notes').length).toBe(2);
  // La fiche de prise de notes (Kobi au repos pendant l'enregistrement).
  const fiche = page.locator('.card-notes');
  await expect(fiche).toBeVisible();
  await fiche.click();
  await expect.poll(async () => (await appels(page)).filter(([c]) => c === 'basculer_notes').length).toBe(3);
});

test('animations réduites : rien ne bouge dans les notes en direct', async ({ page }) => {
  // Réglage « Animations : réduites » de Kobi : la classe sur l'élément racine.
  await page.addInitScript(() => {
    const poser = () => document.documentElement?.classList.add('reduced-motion');
    poser();
    document.addEventListener('readystatechange', poser);
  });
  const errors = await ouvrirNotes(page, NOTES);
  await expect(page.locator('.vue-direct .titre')).toHaveText('Point hebdo');
  const animees = await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length);
  expect(animees).toBe(0);
  await expectNoErrors(errors);
});
