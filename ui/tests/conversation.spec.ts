// Conversation à la voix et au clavier (KOB-133), réunion (KOB-134), tâches
// longues (KOB-135) : l'interface pilotée par les événements de la coque,
// simulée ici (`mockTauri`) tant que la coque n'est pas sur main.
import { expect, test, type Page } from '@playwright/test';
import { calls, emit, expectNoErrors, mockTauri, openUi, sansAnimation } from './helpers';

type Statut = 'attente' | 'reponse' | 'tache' | 'fini' | 'erreur';
const bulle = (tour: number, question: string, reponse: string, statut: Statut, clavier = false) => ({ tour, question, reponse, statut, clavier });

const HISTORIQUE = [
  bulle(1, 'où est géré le refresh token ?', 'Dans api/src/auth/refresh.ts, ligne 88.', 'fini'),
  bulle(2, 'et côté mobile ?', 'Dans app/lib/session.dart, ligne 41.', 'fini', true),
];

/** Boîte d'un élément relevée plusieurs fois : elle ne doit pas bouger. */
async function boxes(page: Page, selector: string): Promise<unknown[]> {
  const out = [];
  for (let i = 0; i < 5; i++) {
    out.push(await page.locator(selector).first().boundingBox());
    // Échantillonnage : la position est relevée toutes les 200 ms.
    await page.waitForTimeout(200);
  }
  return out;
}

test('Entrée envoie le texte tapé (envoyer_texte) : Kobi l’avale et réfléchit aussitôt', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const input = page.getByRole('textbox', { name: 'Écrire à Kobi' });
  // Plus de champ permanent (KOB-147) : la bulle s'ouvre à la première touche.
  await expect(input).toBeHidden();
  await page.keyboard.type('où est géré le refresh token ?');
  await expect(input).toBeVisible();
  await input.press('Enter');
  await expect(input).toHaveValue('');
  // Réaction immédiate, avant toute réponse de la coque.
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'think');
  await expect.poll(() => calls(page)).toContainEqual(['envoyer_texte', { texte: 'où est géré le refresh token ?' }]);
  // Un texte vide n'envoie rien.
  await input.press('Enter');
  // Contrôle négatif : 600 ms laissés à un envoi pour partir.
  await page.waitForTimeout(600);
  expect((await calls(page)).filter(([c]) => c === 'envoyer_texte')).toHaveLength(1);
  await expectNoErrors(errors);
});

test('taper au clavier n’importe où écrit dans le champ', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await page.mouse.click(600, 260);
  await page.keyboard.type('bonjour');
  await expect(page.getByRole('textbox', { name: 'Écrire à Kobi' })).toHaveValue('bonjour');
  await expectNoErrors(errors);
});

test('l’historique (commande historique, événement conversation) se fait défiler dans la bulle', async ({ page }) => {
  await mockTauri(page, { historique: HISTORIQUE });
  const errors = await openUi(page, '');
  await emit(page, 'state', { etat: 'veille', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'answer', 'Avec plaisir.');
  const history = page.getByRole('log', { name: 'Échanges récents avec Kobi' });
  await expect(history).toBeHidden();
  expect(await calls(page)).toContainEqual(['historique', null]);
  // Défiler vers le haut sur la bulle ouvre l'historique, le plus récent en bas.
  // (La carte flotte : on vise sa place à la souris plutôt que d'attendre qu'elle se pose.)
  await page.mouse.move(200, 230);
  await page.mouse.wheel(0, -200);
  await expect(history).toBeVisible();
  await expect(history.locator('.ex')).toHaveCount(2);
  await expect(history.locator('.ex').nth(1).locator('.ex-who')).toHaveText('tapé');
  await expect(history.locator('.ex').nth(0).locator('.ex-who')).toHaveText('dit');
  // Les bulles et les notifications s'effacent le temps de la relecture.
  await expect(page.locator('[data-card=answer]')).toBeHidden();
  // Un nouvel échange arrive pendant la relecture : il s'ajoute.
  await emit(page, 'conversation', [...HISTORIQUE, bulle(3, 'merci', 'Avec plaisir.', 'fini')]);
  await expect(history.locator('.ex')).toHaveCount(3);
  // Échap referme.
  await page.keyboard.press('Escape');
  await expect(history).toBeHidden();
  // Flèche haut dans la bulle vide rouvre ; une nouvelle question referme.
  await page.evaluate(() => window.kobi.ecrire());
  await page.getByRole('textbox', { name: 'Écrire à Kobi' }).press('ArrowUp');
  await expect(history).toBeVisible();
  await emit(page, 'state', { etat: 'veille', activite: 'ecoute', mascotte: 'listen', transcrit: false });
  await expect(history).toBeHidden();
  expect(await calls(page)).not.toContainEqual(['interrompre', null]);
  await expectNoErrors(errors);
});

test('tâche longue : annoncée, Kobi reste disponible, résultat livré en notification puis affiché d’un clic', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const longue = 'liste tous les appels à refresh dans la codebase';
  await emit(page, 'conversation', [bulle(1, longue, '', 'attente')]);
  await emit(page, 'conversation', [bulle(1, longue, 'Je m’en occupe, le résultat arrive.', 'tache')]);
  const running = page.locator('.notif[data-kind=running]');
  await expect(running).toBeVisible();
  await expect(running).toContainText('Je m’en occupe');
  await expect(running).toContainText(longue);
  // Une autre question passe pendant ce temps : la notification reste.
  await page.evaluate(() => window.kobi.ecrire());
  await page.getByRole('textbox', { name: 'Écrire à Kobi' }).fill('quelle heure est-il ?');
  await page.getByRole('textbox', { name: 'Écrire à Kobi' }).press('Enter');
  await emit(page, 'conversation', [bulle(1, longue, 'Je m’en occupe, le résultat arrive.', 'tache'), bulle(2, 'quelle heure est-il ?', 'Il est 10 h 12.', 'fini', true)]);
  await expect(running).toBeVisible();
  // Livraison.
  const resultat = '7 appels : api/src/auth/refresh.ts (3), app/lib/session.dart (2), web/src/auth.ts (2).';
  await emit(page, 'conversation', [bulle(1, longue, resultat, 'fini'), bulle(2, 'quelle heure est-il ?', 'Il est 10 h 12.', 'fini', true)]);
  const delivered = page.getByRole('button', { name: `Résultat prêt : ${longue}` });
  await expect(delivered).toBeVisible();
  await expect(running).toHaveCount(0);
  await expect(delivered).toContainText('7 appels');
  // Une fois arrivée, la notification ne bouge plus : elle se clique du premier coup.
  await sansAnimation(page.locator('.notif').first());
  const b = await boxes(page, '.notif');
  for (const x of b) expect(x).toEqual(b[0]);
  await delivered.click();
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'answer');
  await expect(page.locator('[data-card=answer] .text')).toHaveText(resultat);
  await expect(page.locator('.notif')).toHaveCount(0);
  // Échec d'une tâche.
  await emit(page, 'conversation', [bulle(3, 'compte les fichiers', '', 'tache')]);
  await emit(page, 'conversation', [bulle(3, 'compte les fichiers', 'Claude n’a pas répondu.', 'erreur')]);
  await expect(page.locator('.notif[data-kind=failed]')).toContainText('Tâche en échec');
  await expectNoErrors(errors);
});

test('carte de la coque : Kobi propose, le clic renvoie decider_carte, puis Fait', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'carte', { id: '7', kicker: 'Je crée le ticket ?', title: 'Refresh token à 30 jours', meta: 'Linear · équipe Kobi', status: 'pending' });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'suggest');
  const card = page.locator('[data-card=action]');
  await expect(card).toContainText('Refresh token à 30 jours');
  await card.getByRole('button').first().click();
  expect(await calls(page)).toContainEqual(['decider_carte', { id: '7', accepte: true }]);
  await emit(page, 'carte', { id: '7', kicker: 'Je crée le ticket ?', title: 'Refresh token à 30 jours', meta: null, status: 'confirmed' });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'done');
  // Refusée : retour au calme.
  await emit(page, 'carte', { id: '8', kicker: 'J’envoie le message ?', title: 'Résumé à l’équipe', meta: null, status: 'pending' });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'suggest');
  await emit(page, 'carte', { id: '8', kicker: 'J’envoie le message ?', title: 'Résumé à l’équipe', meta: null, status: 'dismissed' });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'idle');
  await expectNoErrors(errors);
});

test('Échap interrompt la réponse : la frappe s’arrête net et la suite du flux est ignorée (interrompre)', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const full = 'C’est dans api/src/auth/refresh.ts, ligne 88. La durée actuelle est de quinze minutes, et un ticket ouvert en parle déjà depuis lundi dernier.';
  await emit(page, 'state', { etat: 'veille', activite: 'repond', mascotte: 'answer', transcrit: false });
  await emit(page, 'answer', full.slice(0, 60));
  // Voulu : la réponse se tape un moment avant Échap.
  await page.waitForTimeout(700);
  await page.keyboard.press('Escape');
  expect(await calls(page)).toContainEqual(['interrompre', null]);
  const text = page.locator('[data-card=answer] .text');
  const frozen = await text.textContent();
  expect(frozen!.length).toBeLessThan(61);
  // La coque envoie encore la fin du flux : rien ne s'ajoute.
  await emit(page, 'answer', full);
  // Contrôle négatif : 600 ms laissés au texte pour s'ajouter.
  await page.waitForTimeout(600);
  await expect(text).toHaveText(frozen!);
  // Nouvelle question : tout repart.
  await emit(page, 'state', { etat: 'veille', activite: 'ecoute', mascotte: 'listen', transcrit: false });
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'listen');
  await expectNoErrors(errors);
});

test('réunion : la dernière note de l’événement enregistrement s’écrit dans la fiche de Kobi', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  const debut = Date.now() - 125_000;
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: debut, notes: 1, titre: 'Point hebdo', derniere_note: 'Décision : refresh token à 30 jours' });
  await expect(page.locator('.rec')).toBeVisible();
  await expect(page.locator('.stage')).toHaveAttribute('data-state', 'write');
  await expect(page.locator('[data-card=notes] .text')).toHaveText('Décision : refresh token à 30 jours');
  await emit(page, 'enregistrement', { actif: true, pause: false, debut_ms: debut, notes: 2, titre: 'Point hebdo', derniere_note: 'Action : doc API (Léa)' });
  await expect(page.locator('[data-card=notes] .text')).toHaveText('Action : doc API (Léa)');
  await expect(page.locator('.rec-notes')).toContainText('2');
  // On peut toujours écrire à Kobi pendant la réunion.
  await page.keyboard.type('résume');
  await page.getByRole('textbox', { name: 'Écrire à Kobi' }).press('Enter');
  await expect.poll(() => calls(page)).toContainEqual(['envoyer_texte', { texte: 'résume' }]);
  await expectNoErrors(errors);
});

test('toujours animé, sans faire bouger ce qui se clique ; « réduire les animations » coupe tout', async ({ page }) => {
  await mockTauri(page);
  const errors = await openUi(page, '');
  await emit(page, 'conversation', [bulle(1, 'longue tâche', '', 'tache')]);
  await page.evaluate(() => window.kobi.ecrire());
  await page.keyboard.type('bonjour');
  const live = (sel: string) => page.locator(sel).evaluate((n) => n.getAnimations({ subtree: true }).filter((a) => a.playState === 'running').length);
  // La pointe et l'invite ↵ de la bulle vivent (la pointe est un pseudo-élément).
  await expect.poll(() => page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running' && ['ecrire-pointe', 'ecrire-entree'].includes((a as CSSAnimation).animationName)).length)).toBe(2);
  expect(await live('.notif')).toBeGreaterThanOrEqual(3);
  // La bulle (une fois ouverte) et la notification elles-mêmes ne bougent pas.
  await sansAnimation(page.locator('.ecrire'));
  await sansAnimation(page.locator('.notif'));
  const b = await boxes(page, '.ecrire-champ');
  for (const x of b) expect(x).toEqual(b[0]);

  // Réglage « Animations » de Kobi sur « comme Windows » (par défaut : toujours).
  // Windows passe de « animé » à « réduit » en cours de route (quel que soit son état au départ).
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.evaluate(() => localStorage.setItem('kobi.animations', 'systeme'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect(page.locator('.stage')).toHaveClass(/reduced-motion/);
  expect(await live('.ecrire')).toBe(0);
  expect(await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running' && (a as CSSAnimation).animationName?.startsWith('ecrire-')).length)).toBe(0);
  expect(await live('.notif')).toBe(0);
  await expectNoErrors(errors);
});
