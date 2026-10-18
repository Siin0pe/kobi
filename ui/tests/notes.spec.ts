// Notes en direct (KOB-73) avec une fausse coque Tauri. La lecture réelle
// du `notes.md` est testée côté Rust (crates/kobi-app/src/notes.rs).
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

function fakeTauri(): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  w.__notes = null;
  w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  w.__TAURI__ = {
    window: { getCurrentWindow: () => ({ startDragging: async () => {} }) },
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        if (cmd === 'notes_en_cours') return w.__notes;
        if (cmd === 'basculer_notes') return true;
        if (cmd === 'replier_notes' || cmd === 'fermer_notes') return null;
        // La mascotte demande son état au démarrage : rien d'autre ici.
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
  await page.setViewportSize({ width: 360, height: 480 });
  await page.goto('/notes.html');
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

const appels = (page: Page) => page.evaluate(() => (window as unknown as { __appels: Array<[string, unknown]> }).__appels);

test('décisions, actions, questions et dernière note mise en valeur', async ({ page }) => {
  const errors = await ouvrirNotes(page, NOTES);
  await expect(page.locator('.titre')).toHaveText('Point hebdo');
  await expect(page.locator('.duree')).toContainText(/ · 12:0\d/);
  await expect(page.locator('.derniere-texte')).toHaveText('Sophie : devis');
  await expect(page.locator('[data-section="decisions"] li')).toHaveText(['Budget validé', 'Séminaire à Annecy']);
  await expect(page.locator('[data-section="decisions"] .nombre')).toHaveText('2');
  await expect(page.locator('[data-section="actions"] li.tache')).toHaveCount(1);
  await expect(page.locator('[data-section="questions"] .vide')).toHaveText('Aucune question ouverte.');
  await expectNoErrors(errors);
});

test('une nouvelle note arrive : relue sur l’événement, mise en valeur, sections repliées gardées', async ({ page }) => {
  const errors = await ouvrirNotes(page, NOTES);
  await expect(page.locator('.derniere-texte')).toHaveText('Sophie : devis');
  // Section Décisions repliée par l'utilisateur.
  await page.locator('[data-section="decisions"] summary').click();
  await expect(page.locator('[data-section="decisions"]')).not.toHaveAttribute('open', '');
  await page.evaluate(() => {
    const w = window as unknown as { __notes: Record<string, unknown>; __emit: (e: string, p: unknown) => void };
    w.__notes = { ...w.__notes, questions: '- Date du séminaire ?', derniere_note: 'Date du séminaire ?', lignes: 4 };
    w.__emit('notes', '');
  });
  await expect(page.locator('.derniere-texte')).toHaveText('Date du séminaire ?');
  await expect(page.locator('.derniere')).toHaveClass(/arrive/);
  await expect(page.locator('[data-section="questions"] li')).toHaveText(['Date du séminaire ?']);
  await expect(page.locator('[data-section="questions"] li')).toHaveClass(/nouvelle/);
  // Seul le premier affichage entre en cascade : un rafraîchissement ne rejoue rien.
  await expect(page.locator('.sections')).not.toHaveClass(/premier/);
  // Les anciennes puces ne sont pas « nouvelles », la section repliée le reste.
  await expect(page.locator('[data-section="actions"] li.nouvelle')).toHaveCount(0);
  await expect(page.locator('[data-section="decisions"]')).not.toHaveAttribute('open', '');
  await expectNoErrors(errors);
});

test('replier, déplier et fermer le panneau', async ({ page }) => {
  const errors = await ouvrirNotes(page, NOTES);
  await page.getByRole('button', { name: 'Replier' }).click();
  await expect(page.locator('.panneau-notes')).toHaveAttribute('data-replie', 'true');
  await expect(page.locator('.sections')).toBeHidden();
  await expect(page.locator('.derniere-texte')).toBeVisible();
  await page.getByRole('button', { name: 'Déplier' }).click();
  await expect(page.locator('.sections')).toBeVisible();
  await page.getByRole('button', { name: 'Fermer les notes' }).click();
  const faits = await appels(page);
  expect(faits).toContainEqual(['replier_notes', { replie: true }]);
  expect(faits).toContainEqual(['replier_notes', { replie: false }]);
  expect(faits.map(([c]) => c)).toContain('fermer_notes');
  await expectNoErrors(errors);
});

test('avant la première note, puis hors réunion', async ({ page }) => {
  const errors = await ouvrirNotes(page, { ...NOTES, decisions: '', actions: '', derniere_note: null, lignes: 0 });
  await expect(page.locator('.derniere .vide')).toContainText('les premières notes arrivent');
  await page.evaluate(() => {
    const w = window as unknown as { __notes: unknown; __emit: (e: string, p: unknown) => void };
    w.__notes = null;
    w.__emit('enregistrement', { actif: false });
  });
  await expect(page.locator('.titre')).toHaveText('Pas de réunion');
  await expect(page.locator('.derniere .vide')).toContainText('pendant une réunion enregistrée');
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

test('animations réduites : rien ne bouge dans le panneau', async ({ page }) => {
  // Réglage « Animations : réduites » de Kobi : la classe sur l'élément racine.
  await page.addInitScript(() => {
    const poser = () => document.documentElement?.classList.add('reduced-motion');
    poser();
    document.addEventListener('readystatechange', poser);
  });
  const errors = await ouvrirNotes(page, NOTES);
  await expect(page.locator('.titre')).toHaveText('Point hebdo');
  const animees = await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length);
  expect(animees).toBe(0);
  await expectNoErrors(errors);
});
