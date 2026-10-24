// Transcription brute en direct (KOB-50) : outil de mise au point, masqué
// par défaut, sous les notes de la section « En direct ». Ctrl+Alt+T
// l'ouvre ou le ferme (choix retenu). Les segments partiels du moteur sont en
// gris, remplacés au fil de la phrase ; les finaux en clair ; chaque ligne
// commence par sa piste (moi : le micro, eux : le son de l'ordinateur). La
// coque n'envoie rien tant que le panneau n'est pas à l'écran.

import { h } from '../dom';
import { surLangue, t } from '../i18n';
import { disponible, ecouter, invoke } from '../ipc';

/** Ce que publie la coque (`transcription_brute`). */
export interface LigneBrute {
  source: 'moi' | 'eux';
  texte: string;
  finale: boolean;
  /** Partiel resté sans final (nouvel énoncé, pause) : figé tel quel (KOB-274). */
  abandon?: boolean;
}

export interface VueBrute {
  /** La section « En direct » est à l'écran (ou non). */
  montrer(visible: boolean): void;
}

const CLE = 'kobi.transcriptionBrute';
/** Lignes gardées : un outil de mise au point, pas un journal. */
const MAX_LIGNES = 60;
/** Écart (px) sous lequel la liste est « en bas ». */
const BAS = 4;

function lireChoix(): boolean {
  try {
    return localStorage.getItem(CLE) === '1';
  } catch {
    return false;
  }
}

function ecrireChoix(ouvert: boolean): void {
  try {
    localStorage.setItem(CLE, ouvert ? '1' : '0');
  } catch {
    /* stockage indisponible : le choix vaut pour cette ouverture */
  }
}

export function monterTranscriptionBrute(hote: HTMLElement): VueBrute {
  const titre = h('h2', { class: 'brut-titre' });
  const aide = h('span', { class: 'brut-aide' });
  const lignes = h('ol', { class: 'brut-lignes', 'aria-live': 'off' });
  const vide = h('p', { class: 'brut-vide' });
  const panneau = h('section', { class: 'panneau-brut', hidden: true }, h('header', { class: 'brut-tete' }, titre, aide), lignes, vide);
  hote.append(panneau);

  function libeller(): void {
    titre.textContent = t('notes.brut.titre');
    aide.textContent = t('notes.brut.aide');
    vide.textContent = t('notes.brut.vide');
    panneau.setAttribute('aria-label', t('notes.brut.titre'));
    for (const li of lignes.querySelectorAll<HTMLElement>('[data-source]')) {
      li.querySelector('.brut-source')!.textContent = t(`notes.brut.${li.dataset.source}`);
    }
  }

  /** Partiel en cours de chaque piste : remplacé, puis figé par le final. */
  const partiels = new Map<string, HTMLElement>();
  function ajouter(l: LigneBrute): void {
    // Suivi du flux seulement si la liste était en bas : remontée pour
    // relire, elle reste où on l'a laissée (règle de KOB-246, KOB-275).
    const enBas = lignes.scrollTop + lignes.clientHeight >= lignes.scrollHeight - BAS;
    let li = partiels.get(l.source);
    if (!li) {
      li = h('li', { 'data-source': l.source }, h('span', { class: 'brut-source' }, t(`notes.brut.${l.source}`)), h('span', { class: 'brut-texte' }));
      lignes.append(li);
    }
    li.querySelector('.brut-texte')!.textContent = l.texte;
    li.classList.toggle('partiel', !l.finale);
    li.classList.toggle('abandon', Boolean(l.abandon));
    if (l.finale) partiels.delete(l.source);
    else partiels.set(l.source, li);
    while (lignes.children.length > MAX_LIGNES) lignes.firstElementChild!.remove();
    vide.hidden = lignes.children.length > 0;
    if (enBas) lignes.scrollTop = lignes.scrollHeight;
  }
  ecouter<LigneBrute>('transcription_brute', ajouter);

  let ouvert = lireChoix();
  let visible = false;
  let suivi: boolean | null = null;
  /** La coque n'envoie que si le panneau est vraiment à l'écran. */
  function suivre(): void {
    panneau.hidden = !ouvert;
    const actif = ouvert && visible && !document.hidden;
    if (actif === suivi || !disponible()) return;
    suivi = actif;
    void invoke('suivre_transcription', { actif }).catch(() => undefined);
  }
  document.addEventListener('visibilitychange', suivre);
  window.addEventListener('pagehide', () => {
    if (suivi) void invoke('suivre_transcription', { actif: false }).catch(() => undefined);
  });
  document.addEventListener('keydown', (e) => {
    if (!visible || !e.ctrlKey || !e.altKey || e.key.toLowerCase() !== 't') return;
    e.preventDefault();
    ouvert = !ouvert;
    ecrireChoix(ouvert);
    suivre();
  });

  surLangue(libeller);
  libeller();
  vide.hidden = false;
  suivre();

  return {
    montrer(v: boolean) {
      visible = v;
      suivre();
    },
  };
}
