// L'app en anglais (KOB-208) : avec `ui.language = en`, la navigation, le
// Carnet et chaque panneau des Réglages sont en anglais ; un changement de
// langue venu de la coque les retraduit sans relancer.

import { expect, test, type Page } from '@playwright/test';
import { fakeTauri } from './app-fausse-coque';

/** Mots et lettres qui trahissent un texte resté en français. */
const FRANCAIS = /[éèêàùçô«»]|\b(le|la|les|des|du|une|vous|votre|aucun|aucune|réunion|dossier|réglages|choisir)\b/i;

/** Le nom de la langue dans le panneau Langue s'écrit dans cette langue. */
const ENDONYME = /Français/g;

async function ouvrir(page: Page, langue: string | null, ancre: string): Promise<void> {
  await page.addInitScript((l) => ((window as unknown as Record<string, unknown>).__langue = l), langue);
  await page.addInitScript(fakeTauri);
  await page.goto(`/app.html${ancre}`);
}

const changer = (page: Page, l: string | null) =>
  page.evaluate((l) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('langue', l), l);

test('langue (KOB-208) : en anglais, la navigation et chaque panneau des Réglages n’ont plus de français', async ({ page }) => {
  await ouvrir(page, 'en', '#reglages/stockage');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  const ids = await page.locator('.menu-entree').evaluateAll((bs) => bs.map((b) => b.getAttribute('data-panneau')!));
  expect(ids.length).toBeGreaterThan(10);
  const restes: string[] = [];
  for (const id of ids) {
    await page.evaluate((id) => window.kobiApp.aller(`reglages/${id}`), id);
    const feuille = page.locator('.reglages .feuille');
    await expect(feuille.locator('.panneau')).toHaveAttribute('data-panneau', id);
    // Les panneaux lents finissent leur rendu.
    await page.waitForTimeout(150);
    const texte = (await page.locator('.reglages').innerText()) + (await page.locator('.nav-entree').allInnerTexts().then((x) => x.join('\n')));
    for (const ligne of texte.split('\n')) if (FRANCAIS.test(ligne.replace(ENDONYME, ''))) restes.push(`${id} : ${ligne.trim()}`);
  }
  expect([...new Set(restes)]).toEqual([]);
});

/** Données de la fausse coque, écrites en français. */
const DONNEES = /Point budget|Budget validé|Appeler Léa|Point hebdo|Quelle heure est-il/g;

test('langue (KOB-208) : en anglais, les autres sections n’ont plus de français', async ({ page }) => {
  await ouvrir(page, 'en', '#notes');
  const ids = await page.locator('.nav-entree').evaluateAll((bs) => bs.map((b) => b.getAttribute('data-section')!));
  const restes: string[] = [];
  for (const id of ids.filter((x) => x !== 'reglages')) {
    await page.evaluate((id) => window.kobiApp.aller(id), id);
    const vue = page.locator(`section.vue[data-section="${id}"]`);
    await expect(vue).toBeVisible();
    await page.waitForTimeout(150);
    for (const ligne of (await vue.innerText()).split('\n'))
      if (FRANCAIS.test(ligne.replace(DONNEES, ''))) restes.push(`${id} : ${ligne.trim()}`);
  }
  expect([...new Set(restes)]).toEqual([]);
});

test('langue (KOB-208) : le Carnet et la navigation en anglais, puis en français à chaud', async ({ page }) => {
  await ouvrir(page, 'en', '#notes');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(page.locator('.nav-entree[data-section="reglages"]')).toContainText('Settings');
  await expect(page.locator('.nav-entree[data-section="notes"]')).not.toContainText('Carnet');
  await page.evaluate(() => window.kobiApp.aller('reglages/stockage'));
  const titre = page.locator('.reglages .entete .titre');
  await expect(titre).toHaveAttribute('aria-label', 'Storage');
  await changer(page, 'fr');
  await expect(page.locator('html')).toHaveAttribute('lang', 'fr');
  await expect(page.locator('.nav-entree[data-section="reglages"]')).toContainText('Réglages');
  await expect(titre).not.toHaveAttribute('aria-label', 'Storage');
  await expect(page.locator('.reglages .panneau')).toHaveAttribute('data-panneau', 'stockage');
  await changer(page, 'en');
  await expect(titre).toHaveAttribute('aria-label', 'Storage');
});
