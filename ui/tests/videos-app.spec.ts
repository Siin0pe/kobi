// Vidéos de l'app Kobi (KOB-151), une par section : `KOBI_VIDEOS=1 npx
// playwright test videos-app` ; les .webm sont dans test-results/. Rien à
// vérifier ici : les comportements sont testés dans vie-app.spec.ts.
import { test, type Page } from '@playwright/test';
import { fakeTauri } from './app-fausse-coque';

test.skip(!process.env.KOBI_VIDEOS, 'vidéos à la demande (KOBI_VIDEOS=1)');
test.use({ video: { mode: 'on', size: { width: 1180, height: 760 } }, viewport: { width: 1180, height: 760 } });

async function ouvrir(page: Page, ancre: string): Promise<void> {
  await page.addInitScript(fakeTauri);
  await page.goto(`/app.html${ancre}`);
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(1200);
}

const emettre = (page: Page, evenement: string, charge: unknown) =>
  page.evaluate(([e, c]) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit(e as string, c), [evenement, charge] as const);

test('notes et réunions : recherche, réaction du petit Kobi, survols', async ({ page }) => {
  await ouvrir(page, '#notes');
  await page.locator('.vue-notes .recherche input').pressSequentially('budget', { delay: 120 });
  await page.waitForTimeout(1500);
  await page.locator('.vue-notes .recherche input').fill('');
  for (const el of await page.locator('.vue-notes .filtre').all()) {
    await el.hover();
    await page.waitForTimeout(400);
  }
  await page.locator('.vue-notes .element').first().hover();
  await page.waitForTimeout(600);
  await page.locator('.vue-notes .element').first().click();
  await page.waitForTimeout(2000);
});

test('transitions entre toutes les sections', async ({ page }) => {
  await ouvrir(page, '#notes');
  for (const nom of ['Rappels et tâches', 'Activité', 'Réglages', 'À propos', 'Activité', 'Notes et réunions']) {
    await page.getByRole('button', { name: nom, exact: true }).hover();
    await page.waitForTimeout(350);
    await page.getByRole('button', { name: nom, exact: true }).click();
    await page.waitForTimeout(1300);
  }
});

test('rappels : un rappel fait, le petit Kobi « fait ! »', async ({ page }) => {
  await ouvrir(page, '#rappels');
  await page.locator('.vue-rappels .coche').first().hover();
  await page.waitForTimeout(500);
  await page.locator('.vue-rappels .coche').first().click();
  await page.waitForTimeout(2500);
});

test('activité : une nouvelle entrée arrive', async ({ page }) => {
  await ouvrir(page, '#activite');
  await emettre(page, 'journal', { quand_ms: Date.now(), genre: 'action', texte: 'Proposé : cargo test', detail: 'Commande' });
  await page.waitForTimeout(1500);
  await emettre(page, 'journal', { quand_ms: Date.now(), genre: 'validation', texte: 'Accepté : cargo test' });
  await page.waitForTimeout(2000);
});

test('réglages : panneaux, survols immobiles', async ({ page }) => {
  await ouvrir(page, '#reglages');
  for (const el of (await page.locator('.vue-reglages .menu-entree').all()).slice(0, 4)) {
    await el.hover();
    await page.waitForTimeout(300);
    await el.click();
    await page.waitForTimeout(900);
  }
});

test('en direct : bandeau REC, chiffres qui roulent, puis fin de réunion', async ({ page }) => {
  await ouvrir(page, '#notes');
  await page.waitForTimeout(2500);
  await page.locator('.bandeau-rec').click();
  await page.waitForTimeout(2000);
  await emettre(page, 'enregistrement', { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null });
  await page.waitForTimeout(1500);
});

test('petit Kobi : il suit la mascotte, vit au repos', async ({ page }) => {
  await ouvrir(page, '#a-propos');
  for (const m of ['listen', 'think', 'answer', 'done', 'idle']) {
    await emettre(page, 'state', { etat: 'assistant', activite: null, mascotte: m, transcrit: false });
    await page.waitForTimeout(1300);
  }
  await page.waitForTimeout(9000);
});

test('accueil (premier lancement) : arrivée, étapes', async ({ page }) => {
  await ouvrir(page, '#accueil');
  await page.waitForTimeout(2500);
  for (let i = 0; i < 3; i++) {
    const suite = page.getByRole('button', { name: /Continuer|Suivant|C’est parti/ }).first();
    if (!(await suite.isVisible().catch(() => false))) break;
    await suite.hover();
    await page.waitForTimeout(400);
    await suite.click();
    await page.waitForTimeout(1800);
  }
});
