import { test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

// Vidéo de la carte de validation (KOB-62), jointe au ticket : fenêtre native
// (410 × 280) filmée en 820 × 560.
test.use({ viewport: { width: 410, height: 280 }, deviceScaleFactor: 2, video: { mode: 'on', size: { width: 820, height: 560 } } });
// Vidéo pour le ticket seulement, pas en vérification (KOB-172).
test.skip(!process.env.KOBI_PW_VIDEOS, 'vidéo des tickets : KOBI_PW_VIDEOS=1');

const CARTES = [
  {
    id: '1',
    type: 'validation',
    genre: 'fichier',
    resume: 'Modifier le fichier chef.rs',
    detail: {
      diff: '--- crates/kobi-app/src/chef.rs\n+++ crates/kobi-app/src/chef.rs\n@@\n-    intervalle: Duration::from_secs(30),\n+    intervalle: Duration::from_secs(20),\n@@\n-// Notes toutes les 30 s.\n+// Notes toutes les 20 s.',
    },
    echeance_ms: 60000,
  },
  { id: '2', type: 'validation', genre: 'commande', resume: 'Pousser la branche sur GitHub', detail: { commande: 'git push -u origin feat/kob-62', dossier: 'C:\\code\\kobi' }, echeance_ms: 60000 },
  { id: '3', type: 'validation', genre: 'mcp', resume: 'linear · save issue : Devis client', detail: { serveur: 'linear', outil: 'save_issue', params: { title: 'Devis client', team: 'KOB', priority: 2 } }, echeance_ms: 60000 },
  { id: '4', type: 'validation', genre: 'reglage', resume: 'Répondre à voix haute en réunion', detail: { cle: 'voice.meeting', avant: false, apres: true }, echeance_ms: 60000 },
  { id: '5', type: 'validation', genre: 'suppression', resume: 'Supprimer : rm -rf target/release', detail: { commande: 'rm -rf target/release' }, echeance_ms: 12000 },
];

const carte = (page: Page, c: object): Promise<void> => page.evaluate((c) => window.kobi.carte(c as never), c);

test('démonstration de la carte de validation (vidéo jointe au ticket)', async ({ page }) => {
  test.setTimeout(60_000);
  const errors = await openUi(page, '');
  await page.waitForTimeout(800);
  const [fichier, commande, mcp, reglage, suppression] = CARTES;
  // Fichier : on déplie le diff, on valide à la souris.
  await carte(page, fichier!);
  await page.waitForTimeout(1600);
  await page.getByRole('button', { name: 'Valider' }).hover();
  await page.waitForTimeout(500);
  await page.getByRole('button', { name: 'Valider' }).click();
  await page.waitForTimeout(2200);
  // Commande et MCP arrivent ensemble : la seconde attend son tour.
  await carte(page, commande!);
  await carte(page, mcp!);
  await page.waitForTimeout(1800);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(2600);
  await page.keyboard.press('Escape');
  await page.waitForTimeout(2200);
  // Réglage : tranché par la voix (la coque republie la carte).
  await carte(page, reglage!);
  await page.waitForTimeout(1800);
  await carte(page, { ...reglage!, statut: 'acceptee' });
  await page.waitForTimeout(2200);
  // Suppression ignorée : compte à rebours court, refus annoncé.
  await carte(page, suppression!);
  await page.waitForTimeout(14_500);
  await expectNoErrors(errors);
});
