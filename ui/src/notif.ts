// Messages courts en bas de fenêtre (copié, exporté, erreur), lus aussi par
// les lecteurs d'écran.

import { h } from './dom';
import type { Ton } from './reglages/panneau';

export function creerNotifieur(parent: HTMLElement): (texte: string, ton?: Ton) => void {
  const zone = h('div', { class: 'notifs', role: 'status', 'aria-live': 'polite' });
  parent.append(zone);
  return (texte, ton = 'ok') => {
    const n = h('div', { class: 'notif', 'data-ton': ton }, texte);
    zone.append(n);
    window.setTimeout(() => {
      n.classList.add('sort');
      window.setTimeout(() => n.remove(), 300);
    }, ton === 'erreur' ? 6000 : 3000);
  };
}
