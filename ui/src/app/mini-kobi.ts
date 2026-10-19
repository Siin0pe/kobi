// Le petit Kobi de la navigation (KOB-159), synchronisé avec la mascotte :
// mêmes yeux et même mot qu'elle (événement `state`), le mot de la section
// quand Kobi dort et qu'on se promène dans l'app.

import { animationsReduites } from '../dom';
import { invoke, ecouter } from '../ipc';
import { Blinker, Scrambler } from '../mascotte/glyphs';
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

  function poser(): void {
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
  ecouter<SessionState>('state', suivre);
  invoke<SessionState>('etat_session').then(suivre, () => {});
  document.addEventListener('visibilitychange', cligner);
  poser();

  return {
    section(m: string) {
      motSection = m;
      if (etat === 'idle') brouilleur.show('*', m);
    },
  };
}
