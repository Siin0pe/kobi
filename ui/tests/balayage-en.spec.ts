// Balayage en anglais (KOB-211) : avec `ui.language = en`, aucun texte
// français ne reste dans ce que Kobi montre — premier lancement, menu du
// clic droit (dans la page et dans sa fenêtre), vue agrandie, historique,
// cartes de validation, compte rendu, rappel, fil d'activité. Les données
// données à Kobi sont en anglais : tout français trouvé vient de
// l'interface. Les Réglages et les sections de l'app sont balayés par
// langue-app.spec.ts (KOB-208), les états de Kobi par langue-kobi.spec.ts
// (KOB-209).

import { expect, test, type Locator, type Page } from '@playwright/test';
import { fakeTauri } from './app-fausse-coque';
import { expectNoErrors, mockTauri, openUi } from './helpers';

/** Mots et lettres qui trahissent un texte resté en français. */
const FRANCAIS = /[éèêàùçôœ«»]|\b(le|la|les|des|du|une|un|vous|votre|ton|ta|tes|aucun|aucune|réunion|pas|est|avec|pour|dans|sur|et|ou)\b/i;
/** Noms de langue, écrits dans leur langue (panneau et étape Langue). */
const ENDONYMES = /Français/g;

/** Textes visibles et attributs lus (aria-label, title, placeholder) sous `racine`. */
async function textes(racine: Locator): Promise<string[]> {
  return racine.evaluate((r) => {
    const v: string[] = [];
    const marche = document.createTreeWalker(r, NodeFilter.SHOW_TEXT);
    for (let n = marche.nextNode(); n; n = marche.nextNode()) {
      const p = n.parentElement;
      if (!p || p.closest('[hidden], script, style') || !p.getClientRects().length) continue;
      const x = n.textContent!.trim();
      if (x) v.push(x);
    }
    for (const e of [r, ...r.querySelectorAll('*')])
      for (const a of ['aria-label', 'title', 'placeholder']) {
        const x = e.getAttribute(a)?.trim();
        if (x) v.push(x);
      }
    return v;
  });
}

/** Ce qui reste en français sous `racine`, préfixé par `ou`. */
async function francais(racine: Locator, ou: string): Promise<string[]> {
  return (await textes(racine)).filter((x) => FRANCAIS.test(x.replace(ENDONYMES, ''))).map((x) => `${ou} : ${x}`);
}

const emettre = (page: Page, evenement: string, charge: unknown) =>
  page.evaluate(([e, c]) => (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit(e as string, c), [evenement, charge] as const);

test('balayage (KOB-211) : le premier lancement en anglais, chaque étape', async ({ page }) => {
  await page.addInitScript(() => ((window as unknown as Record<string, unknown>).__langue = 'en'));
  await page.addInitScript(fakeTauri);
  await page.goto('/app.html#accueil');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  const accueil = page.locator('.bienvenue');
  await expect(accueil).toBeVisible();
  const n = await page.locator('.bienvenue .etape').count();
  const restes: string[] = [];
  for (let i = 0; i <= n + 1; i++) {
    await page.evaluate((i) => window.bienvenue.aller(i), i);
    await page.waitForTimeout(250);
    restes.push(...(await francais(accueil, `étape ${i}`)));
  }
  expect([...new Set(restes)]).toEqual([]);
  // Le balayage a bien lu l'accueil en anglais.
  await expect(accueil.locator('[data-action="terminer"]')).toHaveText('Let’s go!');
});

test('balayage (KOB-211) : la fenêtre du menu en anglais, hors réunion et en réunion', async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as Record<string, unknown>).__TAURI__ = {
      event: { listen: async () => () => {} },
      core: { invoke: async (cmd: string) => (cmd === 'langue_ui' ? 'en' : null) },
    };
  });
  await page.goto('/menu.html');
  await page.waitForFunction(() => '__ouvrirMenu' in window);
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  // Le titre de la fenêtre aussi (KOB-271).
  await expect(page).toHaveTitle('Kobi menu');
  const restes: string[] = [];
  for (const recording of [false, true]) {
    await page.evaluate(
      (contexte) =>
        (window as unknown as { __ouvrirMenu(o: unknown): Promise<void> }).__ouvrirMenu({
          pointeur: { x: 400, y: 300 },
          zone: { x: 0, y: 0, largeur: 1920, hauteur: 1032 },
          echelle: 1,
          contexte,
        }),
      { recording, paused: false, compact: false, voice: false },
    );
    const menu = page.locator('.kmenu');
    await expect(menu).toBeVisible();
    restes.push(...(await francais(menu, recording ? 'menu en réunion' : 'menu')));
    await expect(menu).toContainText('Quit Kobi');
  }
  expect([...new Set(restes)]).toEqual([]);
});

test('balayage (KOB-211) : vue agrandie, historique, cartes, compte rendu, rappel et activité en anglais', async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 760 });
  await mockTauri(page, { langue_ui: 'en' });
  const errors = await openUi(page, 'reduced=1');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  const scene = page.locator('body');
  const restes: string[] = [];
  const balayer = async (ou: string) => {
    await page.waitForTimeout(300);
    restes.push(...(await francais(scene, ou)));
  };

  // Une conversation : réponse finie, tâche en cours avec ses étapes, erreur.
  const etape = (id: string, type: string, resume: string) => ({ tour: 2, id, outil: type, type, resume, chemin_relatif: null, classe: 'lecture', at_ms: Date.now() });
  await page.evaluate(
    (etapes) =>
      window.kobi.conversation([
        { tour: 1, question: 'what time is it', reponse: 'It is 10:12.', statut: 'fini', clavier: false },
        { tour: 2, question: 'check the build', reponse: '', statut: 'tache', clavier: true, etapes: etapes as never },
        { tour: 3, question: 'open the ticket', reponse: 'Could not reach Linear.', statut: 'erreur', clavier: false },
      ]),
    [etape('a', 'lecture', 'README.md'), etape('b', 'recherche', 'build error')],
  );
  await balayer('historique');
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('kobi:vue', { detail: 'ouvrir' })));
  await balayer('vue agrandie');
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('kobi:vue', { detail: 'fermer' })));

  // Une carte de chaque genre.
  const cartes = [
    { genre: 'fichier', resume: 'Edit notes.md', detail: { diff: '--- a/notes.md\n+++ b/notes.md\n@@ -1 +1 @@\n-old\n+new' } },
    { genre: 'commande', resume: 'Push the branch', detail: { commande: 'git push', dossier: '/code/kobi' } },
    { genre: 'mcp', resume: 'linear · save issue', detail: { serveur: 'linear', outil: 'save_issue', params: { title: 'Quote' } } },
    { genre: 'reglage', resume: 'voice.meeting: false → true', detail: { cle: 'voice.meeting', avant: false, apres: true } },
    { genre: 'suppression', resume: 'rm -rf build', detail: { commande: 'rm -rf build' } },
  ];
  // Toutes envoyées : la première s'ouvre, les autres attendent (badge).
  const envoyer = (c: object) => page.evaluate((c) => window.kobi.carte(c as never), c);
  for (const [i, c] of cartes.entries()) await envoyer({ id: String(i + 1), type: 'validation', echeance_ms: 60_000, ...c });
  const issues = ['acceptee', 'refusee', 'expiree'];
  for (const [i, c] of cartes.entries()) {
    await expect(scene).toContainText(c.resume);
    await balayer(`carte ${c.genre}`);
    // Son issue, puis la suivante.
    await envoyer({ id: String(i + 1), type: 'validation', ...c, statut: issues[i % issues.length] });
    await balayer(`carte ${c.genre} conclue`);
  }

  // Réunion, puis compte rendu prêt.
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: false, debut_ms: Date.now() - 125_000, notes: 3, titre: 'Weekly' } as never));
  await balayer('réunion');
  await page.evaluate(() => window.kobi.recording({ actif: true, pause: true, debut_ms: Date.now() - 125_000, notes: 3, titre: 'Weekly' } as never));
  await balayer('réunion en pause');
  await page.evaluate(() => window.kobi.recording({ actif: false, pause: false, debut_ms: 0, notes: 0, titre: null } as never));
  await page.evaluate(() => window.kobi.compteRendu({ reunion: 'weekly', resume: 'Budget approved.', decisions: 2, actions: 1, questions: 0 }));
  await balayer('compte rendu');
  await expect(scene).toContainText('2 decisions');

  // Rappel, puis une ligne d'activité.
  await page.evaluate(() => window.kobi.rappel({ id: 'r1', texte: 'call Lea', echeance_ms: Date.now() } as never));
  await balayer('rappel');
  await page.evaluate((e) => window.kobi.activite(e as never), { ...etape('c', 'commande', 'cargo test'), classe: 'refus' });
  await balayer('activité');
  // Erreur de la coque (événement `erreur`).
  await emettre(page, 'state', { etat: 'assistant', activite: null, mascotte: 'error', transcrit: false });
  await balayer('erreur');

  expect([...new Set(restes)]).toEqual([]);
  await expectNoErrors(errors);
});
