// Premier lancement (KOB-144) avec une fausse coque Tauri. Les commandes
// réelles sont testées côté Rust (crates/kobi-app/src/bienvenue.rs).
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors } from './helpers';

interface Options {
  claude?: string;
  /** Commandes des dossiers de code présentes (sinon « not found », comme Tauri). */
  dependances?: boolean;
  modele?: boolean;
}

function fakeTauri(o: Options): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  const w = window as unknown as Record<string, unknown>;
  const appels: Array<[string, unknown]> = [];
  w.__appels = appels;
  w.__emit = emit;
  let codebases: string[] = [];
  // Démarrage avec le système, gardé d'un chargement à l'autre (parcours rejoué).
  const demarrage = (): { actif: boolean; choisi: boolean } =>
    JSON.parse(sessionStorage.getItem('kobi-demarrage') ?? '{"actif":false,"choisi":false}');
  const garder = (d: { actif: boolean; choisi: boolean }) => {
    sessionStorage.setItem('kobi-demarrage', JSON.stringify(d));
    return d;
  };
  let modele = o.modele ? '/home/neuf/Kobi/models/sherpa-onnx-nemotron' : null;
  const absente = (cmd: string) => {
    throw `Command ${cmd} not found`;
  };
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
        appels.push([cmd, args]);
        switch (cmd) {
          case 'etat_claude':
            if (o.claude === 'introuvable') return { etat: 'introuvable' };
            if (o.claude === 'non-connecte') return { etat: 'non-connecte', version: '2.1.302 (Claude Code)', programme: '/usr/bin/claude' };
            return { etat: 'pret', version: '2.1.302 (Claude Code)', programme: '/usr/bin/claude' };
          case 'micros':
            return { micros: [{ nom: 'Micro intégré', defaut: true }, { nom: 'Casque USB', defaut: false }], choisi: null, erreur: null };
          case 'choisir_micro':
            return null;
          case 'etat_modele':
            return { dossier: modele, nom: 'sherpa-onnx-nemotron', taille: 475_271_763, en_cours: false };
          case 'telecharger_modele':
            setTimeout(() => emit('modele://progression', { etape: 'telechargement', recu: 100 * 2 ** 20, total: 475_271_763 }), 50);
            setTimeout(() => emit('modele://progression', { etape: 'extraction', recu: 400 * 2 ** 20, total: 475_271_763 }), 150);
            setTimeout(() => {
              modele = '/home/neuf/Kobi/models/sherpa-onnx-nemotron';
              emit('modele://fini', modele);
            }, 300);
            return null;
          case 'annuler_telechargement':
            return null;
          case 'get_codebases':
            return o.dependances ? codebases : absente(cmd);
          case 'set_codebases':
            if (!o.dependances) return absente(cmd);
            codebases = args.dossiers as string[];
            return null;
          case 'choisir_dossier':
            return '/home/neuf/code/kobi';
          case 'demarrage_systeme':
            return demarrage().actif;
          case 'regler_demarrage_systeme':
            return garder({ actif: Boolean(args.actif), choisi: true }).actif;
          case 'demarrage_par_defaut': {
            // Profil neuf : coché par défaut, une seule fois.
            const d = demarrage();
            return (d.choisi ? d : garder({ actif: true, choisi: true })).actif;
          }
          case 'raccourcis':
            return { parler: 'Ctrl+Shift+Space', reunion: 'Ctrl+Shift+R' };
          case 'terminer_bienvenue':
          case 'ouvrir_bienvenue':
            return null;
          default:
            throw new Error(`commande inconnue ${cmd}`);
        }
      },
    },
    event: {
      listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
        (listeners[event] ??= []).push(handler);
        return () => {
          listeners[event] = (listeners[event] ?? []).filter((h) => h !== handler);
        };
      },
    },
  };
}

async function ouvrir(page: Page, o: Options = {}): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await page.addInitScript(fakeTauri, o);
  await page.setViewportSize({ width: 900, height: 640 });
  await page.goto('/app.html#accueil');
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

const appels = (page: Page) => page.evaluate(() => (window as unknown as { __appels: Array<[string, unknown]> }).__appels);
const etape = (page: Page, id: string) => page.locator(`.etape[data-etape="${id}"]`);

test('Kobi se présente, puis le parcours complet mène à « C’est parti »', async ({ page }) => {
  const errors = await ouvrir(page, { dependances: true });
  await expect(page.locator('.titre')).toHaveText('Salut, moi c’est Kobi.');
  await expect(page.locator('.bulle')).toContainText('je ne fais que lire ton code');
  // Les étapes sont vérifiées d'emblée.
  await expect(etape(page, 'claude')).toHaveAttribute('data-statut', 'ok');
  await expect(etape(page, 'modele')).toHaveAttribute('data-statut', 'attention');
  await expect(etape(page, 'modele').locator('.etape-resume')).toHaveText('à télécharger');

  await page.getByRole('button', { name: 'On y va' }).click();
  // 1. Claude Code.
  await expect(page.locator('.surtitre')).toHaveText('Étape 1 sur 7');
  await expect(page.locator('.etat[data-statut="ok"]')).toContainText('Claude Code 2.1.302 est installé et connecté.');
  await page.getByRole('button', { name: 'Suivant' }).click();
  // 2. Micro.
  await page.getByRole('radio', { name: /Casque USB/ }).click();
  await expect(page.getByRole('radio', { name: /Casque USB/ })).toHaveAttribute('aria-checked', 'true');
  expect(await appels(page)).toContainEqual(['choisir_micro', { nom: 'Casque USB' }]);
  await page.getByRole('button', { name: 'Suivant' }).click();
  // 3. Modèle : taille annoncée, progression, installé.
  await page.getByRole('button', { name: 'Télécharger (453 Mo)' }).click();
  await expect(page.locator('.etat[data-statut="ok"]')).toContainText('Le modèle de transcription est installé.');
  await expect(etape(page, 'modele')).toHaveAttribute('data-statut', 'ok');
  await page.getByRole('button', { name: 'Suivant' }).click();
  // 4. Dossiers de code.
  await page.getByRole('button', { name: 'Ajouter un dossier…' }).click();
  await expect(page.locator('.dossier-code code')).toHaveText('/home/neuf/code/kobi');
  expect(await appels(page)).toContainEqual(['set_codebases', { dossiers: ['/home/neuf/code/kobi'] }]);
  expect(await appels(page)).toContainEqual(['choisir_dossier', { titre: 'Dossier de code à lire' }]);
  await page.getByRole('button', { name: 'Suivant' }).click();
  // 5. Démarrage avec le système : coché par défaut au premier lancement.
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('button', { name: 'Suivant' }).click();
  // 6. Essai : la question transcrite puis la réponse de Kobi.
  await expect(page.locator('.essai-consigne')).toHaveText('Dis : « Kobi, bonjour »');
  await page.evaluate(() => {
    const emit = (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit;
    emit('question', 'Kobi, bonjour');
    emit('answer', 'Bonjour ! Je suis prêt.');
  });
  await expect(page.locator('.essai-question')).toHaveText('« Kobi, bonjour »');
  await expect(page.locator('.essai-reponse')).toHaveText('Bonjour ! Je suis prêt.');
  await expect(etape(page, 'essai')).toHaveAttribute('data-statut', 'ok');
  await page.getByRole('button', { name: 'Suivant' }).click();
  // 7. Raccourcis.
  await expect(page.locator('.raccourcis kbd')).toHaveText(['Ctrl', 'Maj', 'Espace', 'Ctrl', 'Maj', 'R']);
  await page.getByRole('button', { name: 'Terminer' }).click();
  // Fin.
  await expect(page.locator('.titre')).toHaveText('À toi de jouer !');
  await expect(page.locator('.panneau')).toContainText('Tout est en ordre.');
  await page.getByRole('button', { name: 'C’est parti !' }).click();
  expect((await appels(page)).map(([c]) => c)).toContain('terminer_bienvenue');
  await expectNoErrors(errors);
});

test('Claude non connecté : la marche à suivre, puis revérifier', async ({ page }) => {
  const errors = await ouvrir(page, { claude: 'non-connecte' });
  await page.getByRole('button', { name: 'On y va' }).click();
  await expect(page.locator('.etat[data-statut="attention"]')).toContainText('pas connecté');
  await expect(page.locator('.etat code')).toHaveText('claude auth login');
  await expect(etape(page, 'claude')).toHaveAttribute('data-statut', 'attention');
  await page.getByRole('button', { name: 'Revérifier' }).click();
  await expect(page.locator('.etat[data-statut="attention"]')).toBeVisible();
  await expectNoErrors(errors);
});

test('une commande absente : l’étape le dit et ne bloque pas le parcours', async ({ page }) => {
  const errors = await ouvrir(page, { modele: true });
  await expect(etape(page, 'dossiers-code')).toHaveAttribute('data-statut', 'attente');
  await expect(etape(page, 'demarrage')).toHaveAttribute('data-statut', 'ok');
  await page.locator('.etape[data-etape="dossiers-code"] button').click();
  await expect(page.locator('.etat[data-statut="attente"]')).toContainText('prochaine version');
  await page.getByRole('button', { name: 'Suivant' }).click();
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'true');
  // La fin ne liste que ce qui est à revoir : « en attente » n'en fait pas partie.
  await page.evaluate(() => window.bienvenue.aller(8));
  await expect(page.locator('.panneau')).toContainText('Tout est en ordre.');
  await expectNoErrors(errors);
});

test('profil neuf : le démarrage avec le système est coché et activé par défaut', async ({ page }) => {
  const errors = await ouvrir(page, { dependances: true, modele: true });
  await expect(etape(page, 'demarrage').locator('.etape-resume')).toHaveText('activé');
  await page.locator('.etape[data-etape="demarrage"] button').click();
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'true');
  const faits = (await appels(page)).map(([c]) => c);
  expect(faits).toContain('demarrage_par_defaut');
  expect(faits).not.toContain('regler_demarrage_systeme');
  expect(await page.evaluate(() => sessionStorage.getItem('kobi-demarrage'))).toBe('{"actif":true,"choisi":true}');
  await expectNoErrors(errors);
});

test('décoché puis parcours rejoué : le démarrage reste décoché', async ({ page }) => {
  const errors = await ouvrir(page, { dependances: true, modele: true });
  await page.locator('.etape[data-etape="demarrage"] button').click();
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('switch').click();
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'false');
  // Le parcours est rejoué (Réglages → Premier lancement) : nouvelle fenêtre.
  await page.reload();
  await expect(etape(page, 'demarrage').locator('.etape-resume')).toHaveText('désactivé');
  await page.locator('.etape[data-etape="demarrage"] button').click();
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'false');
  expect(await page.evaluate(() => sessionStorage.getItem('kobi-demarrage'))).toBe('{"actif":false,"choisi":true}');
  await expectNoErrors(errors);
});

test('la fenêtre Réglages montre l’état sans rien activer d’office', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri, { dependances: true, modele: true });
  await page.goto('/reglages.html?panneau=demarrage');
  await expect(page.getByRole('switch')).toHaveAttribute('aria-checked', 'false');
  const faits = (await appels(page)).map(([c]) => c);
  expect(faits).toContain('demarrage_systeme');
  expect(faits).not.toContain('demarrage_par_defaut');
  await expectNoErrors(errors);
});

test('essai sans rien entendu : Kobi le dit et propose le raccourci', async ({ page }) => {
  await page.clock.install();
  const errors = await ouvrir(page, { dependances: true, modele: true });
  await page.evaluate(() => window.bienvenue.aller(6));
  await expect(page.locator('.essai-etat')).toContainText('J’écoute…');
  await page.clock.runFor(21_000);
  await expect(page.locator('.essai-etat')).toContainText('Je n’ai rien entendu.');
  await expect(page.locator('.essai-etat .touches')).toHaveAttribute('aria-label', 'Ctrl + Maj + Espace');
  await expect(etape(page, 'essai')).toHaveAttribute('data-statut', 'attention');
  // À la fin, l'essai est listé à revoir.
  await page.evaluate(() => window.bienvenue.aller(8));
  await expect(page.locator('.a-revoir')).toContainText('Essai');
  await expectNoErrors(errors);
});

test('la fenêtre Réglages monte les mêmes étapes et rejoue le parcours', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri, { dependances: true, modele: true });
  await page.goto('/app.html#reglages/micro');
  await expect(page.locator('.titre')).toHaveText('Micro');
  await expect(page.getByRole('radio', { name: /Celui du système/ })).toHaveAttribute('aria-checked', 'true');
  await page.locator('.menu-entree[data-panneau="premier-lancement"]').click();
  await page.getByRole('button', { name: 'Rejouer le premier lancement' }).click();
  expect((await appels(page)).map(([c]) => c)).toContain('ouvrir_bienvenue');
  await expectNoErrors(errors);
});

test('animations réduites : Kobi et les titres posés sans brouillage', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const errors = await ouvrir(page, { dependances: true, modele: true });
  expect(await page.locator('.titre').textContent()).toBe('Salut, moi c’est Kobi.');
  expect(await page.locator('.grand-kobi .mot').textContent()).toBe('salut');
  const animees = await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length);
  expect(animees).toBe(0);
  await expectNoErrors(errors);
});
