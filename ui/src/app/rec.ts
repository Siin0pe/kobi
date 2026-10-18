// Bandeau REC de l'app (KOB-159) : pendant une réunion enregistrée, en haut
// de toutes les sections, avec la durée et le titre ; un clic mène aux notes
// en direct. Suit `etat_enregistrement` puis l'événement `enregistrement`.

import { h } from '../dom';
import { ecouter, invoke } from '../ipc';
import type { RecordingState } from '../recording';

/** Durée écoulée depuis `debut_ms`, en `m:ss` ou `h:mm:ss`. */
export function chrono(debut_ms: number, maintenant = Date.now()): string {
  const s = Math.max(0, Math.floor((maintenant - debut_ms) / 1000));
  const hh = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(hh ? 2 : 1, '0');
  const ss = String(s % 60).padStart(2, '0');
  return `${hh ? `${hh}:` : ''}${mm}:${ss}`;
}

export interface BandeauRec {
  element: HTMLElement;
  /** La réunion en cours, `null` hors réunion. */
  courant(): RecordingState | null;
}

/**
 * `ouvrir` mène aux notes en direct ; `change` est appelé quand une réunion
 * commence ou finit (la navigation montre alors l'entrée « En direct »).
 */
export function monterBandeauRec(ouvrir: () => void, change: (r: RecordingState | null) => void): BandeauRec {
  const duree = h('span', { class: 'rec-duree' });
  const titre = h('span', { class: 'rec-titre' });
  const etat = h('span', { class: 'rec-etat' });
  const element = h(
    'button',
    { type: 'button', class: 'bandeau-rec', hidden: true, 'aria-label': 'Réunion en cours : voir les notes en direct', onclick: ouvrir },
    h('span', { class: 'rec-point', 'aria-hidden': 'true' }),
    h('span', { class: 'rec-mot' }, 'REC'),
    duree,
    titre,
    etat,
    h('span', { class: 'rec-lien' }, 'Notes en direct →'),
  );

  let r: RecordingState | null = null;
  let minuterie = 0;
  const tic = () => {
    if (r) duree.textContent = chrono(r.debut_ms);
  };
  const suivre = () => {
    window.clearInterval(minuterie);
    if (r && !r.pause && !document.hidden) minuterie = window.setInterval(tic, 1000);
  };

  function poser(nouveau: RecordingState | null | undefined): void {
    const avant = r !== null;
    r = nouveau?.actif ? nouveau : null;
    element.hidden = !r;
    element.dataset.pause = String(!!r?.pause);
    if (r) {
      titre.textContent = r.titre ?? 'Réunion';
      etat.textContent = r.pause ? 'en pause' : `${r.notes} ${r.notes > 1 ? 'notes' : 'note'}`;
      tic();
    }
    suivre();
    if (avant !== (r !== null)) change(r);
  }

  ecouter<RecordingState>('enregistrement', poser);
  invoke<RecordingState>('etat_enregistrement').then(poser, () => {});
  document.addEventListener('visibilitychange', suivre);
  return { element, courant: () => r };
}
