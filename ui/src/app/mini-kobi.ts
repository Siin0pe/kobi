// Le petit Kobi de la navigation (KOB-159), synchronisé avec la mascotte :
// mêmes yeux et même mot qu'elle (événement `state`), le mot de la section
// quand Kobi dort et qu'on se promène dans l'app. Au repos, il vit (regards,
// clins d'œil, une lettre qui se brouille) et réagit à ce qu'on fait dans
// l'app (KOB-151) : recherche, rappel fait, suppression, réglage enregistré,
// erreur. Les sections le préviennent par `reagir(humeur)`.

import { animationsReduites } from '../dom';
import { invoke, ecouter } from '../ipc';
import { Blinker, Scrambler, type Eyes } from '../mascotte/glyphs';
import type { SessionState } from '../native';
import { isMascotState, type MascotState } from '../states';

/**
 * Yeux et mot de chaque état : les mêmes que `LOOKS` dans
 * `mascotte/kobi.ts` (non importé : il entraîne toute la scène de la
 * mascotte et sa feuille de styles).
 */
export const REGARDS: Record<MascotState, { oeil: string; mot: string }> = {
  idle: { oeil: '–', mot: 'zzz' },
  listen: { oeil: 'O', mot: 'oui ?' },
  think: { oeil: '@', mot: 'hmm…' },
  write: { oeil: 'o', mot: 'noté' },
  answer: { oeil: '^', mot: 'alors' },
  suggest: { oeil: '*', mot: 'idée !' },
  done: { oeil: '^', mot: 'fait !' },
  error: { oeil: 'x', mot: 'oups' },
  work: { oeil: '#', mot: 'occupé' },
  compose: { oeil: 'o', mot: 'je rédige' },
};

/** États où le petit Kobi cligne des yeux (comme la mascotte). */
const CLIGNE: ReadonlySet<MascotState> = new Set(['idle', 'listen', 'write', 'answer', 'done', 'compose']);

/** Ce que l'app fait et à quoi le petit Kobi réagit. */
export type Humeur = 'cherche' | 'fait' | 'supprime' | 'enregistre' | 'erreur';

const REACTIONS: Record<Humeur, { oeil: string; mot: string }> = {
  cherche: { oeil: '@', mot: 'hmm…' },
  fait: { oeil: '^', mot: 'fait !' },
  supprime: { oeil: 'o', mot: 'pouf !' },
  enregistre: { oeil: '^', mot: 'noté' },
  erreur: { oeil: 'x', mot: 'oups' },
};

/** Durée d'une réaction avant le retour au repos. */
const REACTION_MS = 1_400;

/** Petites vies du repos : expression passagère des deux yeux. */
const VIES: ReadonlyArray<readonly Eyes[]> = [
  [['<', '<'], ['>', '>']], // regarde autour
  [['*', '–']], // clin d'œil
  [['°', '°']], // regarde en l'air
  [['^', '^']], // sourit
];

/** Événement DOM par lequel une section prévient le petit Kobi. */
const EVENEMENT = 'kobi:reaction';

/** Prévient le petit Kobi de l'app (sans effet hors de l'app). */
export function reagir(humeur: Humeur): void {
  window.dispatchEvent(new CustomEvent<Humeur>(EVENEMENT, { detail: humeur }));
}

export interface MiniKobi {
  /** On arrive sur une section : son mot, si Kobi n'est pas occupé. */
  section(mot: string): void;
}

export function monterMiniKobi(racine: HTMLElement, yeux: HTMLElement[], mot: HTMLElement): MiniKobi {
  const brouilleur = new Scrambler(yeux, mot);
  brouilleur.setInstant(animationsReduites());
  const blinker = new Blinker((ferme) => brouilleur.blink(ferme));
  let etat: MascotState = 'idle';
  let motSection = '';

  const cligner = () => blinker.run(CLIGNE.has(etat) && !animationsReduites() && !document.hidden);

  let reaction = 0;

  function poser(): void {
    window.clearTimeout(reaction);
    reaction = 0;
    delete racine.dataset.reaction;
    racine.dataset.etat = etat;
    // Au repos, Kobi regarde l'app avec toi.
    if (etat === 'idle') brouilleur.show('*', motSection);
    else brouilleur.show(REGARDS[etat].oeil, REGARDS[etat].mot);
    cligner();
  }

  const suivre = (s: SessionState | null | undefined) => {
    if (!s || !isMascotState(s.mascotte)) return;
    // En réunion, la mascotte au calme prend des notes (fiche « noté »).
    const vu = s.mascotte === 'idle' && s.etat === 'reunion' ? 'write' : s.mascotte;
    if (vu === etat) return;
    etat = vu;
    poser();
  };
  // Réactions : seulement au repos (la mascotte occupée a la priorité).
  window.addEventListener(EVENEMENT, (e) => {
    const humeur = (e as CustomEvent<Humeur>).detail;
    const r = REACTIONS[humeur];
    if (!r || etat !== 'idle') return;
    window.clearTimeout(reaction);
    brouilleur.show(r.oeil, r.mot);
    racine.dataset.reaction = humeur;
    reaction = window.setTimeout(poser, REACTION_MS);
  });

  // Vie au repos : toutes les 4 à 9 s, un regard, un clin d'œil ou une
  // lettre du mot qui se brouille. En pause fenêtre masquée.
  let vie = 0;
  racine.addEventListener('animationend', (e) => {
    if (e.target === racine && e.animationName === 'app-respire') delete racine.dataset.souffle;
  });
  const vivre = () => {
    window.clearTimeout(vie);
    if (document.hidden) return;
    vie = window.setTimeout(() => {
      // Un souffle à chaque fois, et une petite expression.
      if (!animationsReduites() && !racine.dataset.reaction) {
        delete racine.dataset.souffle;
        void racine.offsetWidth;
        racine.dataset.souffle = '';
      }
      if (etat === 'idle' && !reaction && !animationsReduites() && !brouilleur.scrambling) {
        const n = Math.floor(Math.random() * (VIES.length + 1));
        const suite = VIES[n];
        if (!suite) brouilleur.glitch(Math.floor(Math.random() * motSection.length));
        else
          suite.forEach((yeux, i) => window.setTimeout(() => brouilleur.express(yeux), i * 450));
        if (suite) window.setTimeout(() => brouilleur.express(null), suite.length * 450 + 350);
      }
      vivre();
    }, 4_000 + Math.random() * 5_000);
  };

  ecouter<SessionState>('state', suivre);
  invoke<SessionState>('etat_session').then(suivre, () => {});
  document.addEventListener('visibilitychange', () => {
    cligner();
    vivre();
  });
  vivre();
  poser();

  return {
    section(m: string) {
      motSection = m;
      if (etat === 'idle') brouilleur.show('*', m);
    },
  };
}
