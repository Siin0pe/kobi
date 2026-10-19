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

/** En plus du message : un aperçu (premières lignes copiées) et une action (« Ouvrir »). */
export interface Plus {
  apercu?: string;
  action?: { libelle: string; faire: () => void };
}

export type Notifieur = (texte: string, ton?: Ton, plus?: Plus) => void;

export function creerNotifieur(parent: HTMLElement): Notifieur {
  const zone = h('div', { class: 'notifs', role: 'status', 'aria-live': 'polite' });
  parent.append(zone);
  let timer = 0;
  const partir = (n: HTMLElement): void => {
    n.classList.add('sort');
    window.setTimeout(() => n.remove(), 300);
  };
  return (texte, ton = 'ok', plus = {}) => {
    window.clearTimeout(timer);
    for (const ancien of zone.querySelectorAll<HTMLElement>('.notif:not(.sort)')) partir(ancien);
    const n = h('div', { class: 'notif', 'data-ton': ton }, h('span', { class: 'notif-texte' }, texte));
    if (plus.apercu) n.append(h('span', { class: 'notif-apercu' }, plus.apercu));
    if (plus.action) {
      const { libelle, faire } = plus.action;
      n.classList.add('avec-action');
      n.append(h('button', { type: 'button', class: 'notif-action', onclick: () => (faire(), partir(n)) }, libelle));
    }
    zone.append(n);
    // L'aperçu se lit aussi : il compte dans la durée.
    timer = window.setTimeout(() => partir(n), dureeNotif(`${texte} ${plus.apercu ?? ''}`.trim(), ton) + (plus.action ? 3000 : 0));
  };
}
