// Premier lancement (KOB-144), écran d'accueil de l'app Kobi au premier
// démarrage (KOB-159) : Kobi se présente, puis passe les étapes du parcours
// UX (Claude Code, micro, modèle, dossiers de code, démarrage, essai
// « Kobi, bonjour », raccourcis). Chaque étape est un panneau des réglages
// (`reglages/etapes.ts`) : on peut les revoir dans la section Réglages, et
// rejouer ce parcours depuis « Premier lancement ». Rien ne bloque : chaque
// étape peut être passée.

import './bienvenue.css';
import { animationsReduites, h } from '../dom';
import { disponible, invoke, message } from '../ipc';
import { Blinker, Scrambler } from '../mascotte/glyphs';
import type { Panneau, Statut, Ton } from '../reglages/panneau';
import { ETAPES } from '../reglages/panneaux';
import { poserTitre } from '../titre';

/** Mot et yeux de Kobi pour chaque étape. */
const VISAGES: Record<string, [string, string]> = {
  intro: ['*', 'salut'],
  claude: ['o', 'claude'],
  micro: ['o', 'micro'],
  modele: ['o', 'modèle'],
  'dossiers-code': ['o', 'code'],
  demarrage: ['o', 'réveil'],
  essai: ['^', 'bonjour'],
  raccourcis: ['o', 'clavier'],
  fin: ['^', 'prêt !'],
};
/** Yeux selon l'état de l'étape. */
const YEUX: Record<Statut, string> = { ok: '^', attention: 'o', erreur: 'x', attente: '-' };

/** Monte l'écran d'accueil dans `racine` ; `fini` est appelé au « C'est parti ! ». */
export function monterAccueil(
  racine: HTMLElement,
  { fini, notifier }: { fini: () => void; notifier: (texte: string, ton?: Ton) => void },
): void {
  const statuts = new Map<string, { statut: Statut; resume: string }>();

  // ----- Squelette -----

  const yeux = [h('span', { class: 'g' }, '*'), h('span', { class: 'g' }, '*')];
  const mot = h('div', { class: 'mot', 'aria-hidden': 'true' });
  const kobi = h('div', { class: 'grand-kobi', 'aria-hidden': 'true' }, h('div', { class: 'yeux' }, ...yeux), mot);
  const brouilleur = new Scrambler(yeux, mot);
  brouilleur.setInstant(animationsReduites());
  const blinker = new Blinker((ferme) => brouilleur.blink(ferme));
  blinker.run(!animationsReduites());

  const liste = h('ol', { class: 'etapes' });
  const surtitre = h('p', { class: 'surtitre' });
  const titre = h('h1', { class: 'titre' });
  const sousTitre = h('p', { class: 'sous-titre' });
  const corps = h('div', { class: 'panneau' });
  const retour = h('button', { type: 'button', class: 'bouton', 'data-action': 'retour', onclick: () => aller(courant - 1) }, 'Retour');
  const suivant = h('button', { type: 'button', class: 'bouton plein', 'data-action': 'suivant' }, 'Suivant');
  const scene = h('main', { class: 'scene' }, h('article', { class: 'feuille' }, h('header', { class: 'entete' }, surtitre, titre, sousTitre), corps), h('footer', { class: 'pied' }, retour, suivant));

  racine.append(
    h('div', { class: 'bienvenue' }, h('aside', { class: 'rail' }, kobi, liste), scene),
  );

  ETAPES.forEach((p, i) => {
    liste.append(
      h(
        'li',
        { class: 'etape', 'data-etape': p.id, 'data-statut': 'vide', style: `--i:${i}` },
        h(
          'button',
          { type: 'button', onclick: () => aller(i + 1) },
          h('span', { class: 'pastille', 'aria-hidden': 'true' }),
          h('span', { class: 'etape-corps' }, h('span', { class: 'etape-titre' }, p.titre), h('span', { class: 'etape-resume' })),
        ),
      ),
    );
  });

  function signaler(id: string, statut: Statut, resume: string): void {
    statuts.set(id, { statut, resume });
    const li = liste.querySelector<HTMLElement>(`[data-etape="${id}"]`);
    if (li) {
      li.dataset.statut = statut;
      li.querySelector('.etape-resume')!.textContent = resume;
    }
    if (ETAPES[courant - 1]?.id === id) {
      brouilleur.express(null);
      const [, m] = VISAGES[id] ?? ['o', id];
      if (!brouilleur.scrambling) brouilleur.show(YEUX[statut], m);
    }
  }

  // ----- Navigation -----

  /** 0 : présentation ; 1..n : étapes ; n + 1 : fin. */
  let courant = -1;
  let demonter: void | (() => void);

  function aller(n: number): void {
    n = Math.max(0, Math.min(ETAPES.length + 1, n));
    if (n === courant) return;
    if (typeof demonter === 'function') demonter();
    demonter = undefined;
    courant = n;
    [...liste.children].forEach((li, i) => {
      const b = li.querySelector('button')!;
      if (i === n - 1) b.setAttribute('aria-current', 'step');
      else b.removeAttribute('aria-current');
    });
    scene.dataset.page = n === 0 ? 'intro' : n > ETAPES.length ? 'fin' : 'etape';
    retour.hidden = n === 0;
    corps.classList.remove('entre');
    void corps.offsetWidth;
    corps.classList.add('entre');
    if (n === 0) return presentation();
    if (n > ETAPES.length) return fin();
    const p = ETAPES[n - 1]!;
    const [oeil, m] = VISAGES[p.id] ?? ['o', p.id];
    brouilleur.show(oeil, m);
    surtitre.textContent = `Étape ${n} sur ${ETAPES.length}`;
    poserTitre(titre, p.titre);
    sousTitre.textContent = p.sousTitre;
    suivant.textContent = n === ETAPES.length ? 'Terminer' : 'Suivant';
    suivant.dataset.action = 'suivant';
    corps.dataset.panneau = p.id;
    demonter = p.monter(corps, { notifier, signaler: (s, r) => signaler(p.id, s, r), parcours: true });
  }

  function presentation(): void {
    brouilleur.show(...VISAGES.intro!);
    surtitre.textContent = 'Premier lancement';
    poserTitre(titre, 'Salut, moi c’est Kobi.');
    sousTitre.textContent = '';
    corps.dataset.panneau = 'intro';
    corps.replaceChildren(
      h('div', { class: 'bulle' }, 'Je t’écoute sur cet ordinateur, je réponds avec ton Claude Code, et pour l’instant je ne fais que lire ton code.'),
      h('p', { class: 'intro' }, `On règle ${ETAPES.length} petites choses ensemble, ça prend deux minutes. Tout se retrouve ensuite dans les réglages.`),
    );
    suivant.textContent = 'On y va';
    suivant.dataset.action = 'commencer';
  }

  function fin(): void {
    brouilleur.show(...VISAGES.fin!);
    surtitre.textContent = 'C’est prêt';
    poserTitre(titre, 'À toi de jouer !');
    sousTitre.textContent = '';
    corps.dataset.panneau = 'fin';
    const aRevoir = ETAPES.filter((p) => {
      const s = statuts.get(p.id)?.statut;
      return s === 'attention' || s === 'erreur';
    });
    corps.replaceChildren(
      h('p', { class: 'intro' }, 'Dis « Kobi » quand tu as besoin de moi. Je reste en bas à droite de l’écran ; clic droit sur moi pour le Carnet et les réglages.'),
      aRevoir.length
        ? h('div', { class: 'a-revoir' }, h('p', {}, 'À revoir quand tu veux, dans les réglages :'), h('ul', {}, ...aRevoir.map((p) => h('li', {}, h('strong', {}, p.titre), ` · ${statuts.get(p.id)!.resume}`))))
        : h('p', { class: 'aide' }, 'Tout est en ordre.'),
    );
    suivant.textContent = 'C’est parti !';
    suivant.dataset.action = 'terminer';
  }

  suivant.addEventListener('click', () => {
    if (suivant.dataset.action === 'terminer')
      invoke('terminer_bienvenue').then(fini, (e) => notifier(message(e), 'erreur'));
    else aller(courant + 1);
  });

  window.addEventListener('keydown', (e) => {
    if (!racine.isConnected || racine.closest('[hidden]')) return;
    const cible = e.target as HTMLElement | null;
    if (cible?.closest('input, textarea')) return;
    if (e.key === 'ArrowRight' && e.altKey) aller(courant + 1);
    if (e.key === 'ArrowLeft' && e.altKey) aller(courant - 1);
  });

  /** Vérifie d'emblée les étapes (sauf l'essai) pour remplir la liste. */
  function precontroler(): void {
    for (const p of ETAPES as Panneau[]) {
      if (p.id === 'essai') continue;
      const hors = document.createElement('div');
      const arret = p.monter(hors, { notifier: () => {}, signaler: (s, r) => signaler(p.id, s, r), parcours: true });
      if (typeof arret === 'function') window.setTimeout(arret, 15_000);
    }
  }

  window.bienvenue = { aller };

  aller(0);
  if (disponible()) precontroler();
  else notifier('Ouvrez le premier lancement depuis Kobi.', 'erreur');
}

/** Accès pour les tests. */
declare global {
  interface Window {
    bienvenue: { aller(n: number): void };
  }
}
