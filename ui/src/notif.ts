// Messages courts en bas de fenêtre (copié, exporté, erreur), lus aussi par
// les lecteurs d'écran.
//
// Un seul à la fois, comme les notifications de Kobi (KOB-215) : un nouveau
// message remplace le précédent au lieu de s'empiler ; il reste le temps de
// se lire (selon sa longueur), deux fois plus pour une erreur.

import { h } from './dom';
import type { Ton } from './reglages/panneau';

/** Durée d'affichage : de quoi lire le texte, entre 2,5 et 8 s (le double pour une erreur). */
export function dureeNotif(texte: string, ton: Ton = 'ok'): number {
  const lecture = Math.min(8000, Math.max(2500, 1500 + texte.length * 50));
  return ton === 'erreur' ? lecture * 2 : lecture;
}

export function creerNotifieur(parent: HTMLElement): (texte: string, ton?: Ton) => void {
  const zone = h('div', { class: 'notifs', role: 'status', 'aria-live': 'polite' });
  parent.append(zone);
  let timer = 0;
  const partir = (n: HTMLElement): void => {
    n.classList.add('sort');
    window.setTimeout(() => n.remove(), 300);
  };
  return (texte, ton = 'ok') => {
    window.clearTimeout(timer);
    for (const ancien of zone.querySelectorAll<HTMLElement>('.notif:not(.sort)')) partir(ancien);
    const n = h('div', { class: 'notif', 'data-ton': ton }, texte);
    zone.append(n);
    timer = window.setTimeout(() => partir(n), dureeNotif(texte, ton));
  };
}
