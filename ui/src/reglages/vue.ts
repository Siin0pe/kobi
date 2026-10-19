// Section « Réglages » de l'app Kobi (KOB-159) : la liste des panneaux à
// gauche, le panneau choisi à droite. Les panneaux viennent de `PANNEAUX`
// (ui/src/reglages/panneaux.ts) : un nouvel écran s'y ajoute sans toucher
// à l'app (voir `ajouterPanneau`).

import { h } from '../dom';
import type { Ton } from './panneau';
import { PANNEAUX, panneau } from './panneaux';
import { poserTitre } from '../titre';

export interface VueReglages {
  /** Montre un panneau (le premier si `id` est inconnu). */
  montrer(id?: string | null): void;
}

/** Monte la section dans `racine`. `notifier` affiche les messages courts de l'app. */
export function monterReglages(racine: HTMLElement, notifier: (texte: string, ton?: Ton) => void): VueReglages {
  const nav = h('nav', { class: 'menu', 'aria-label': 'Réglages' });
  const titre = h('h1', { class: 'titre' });
  const sousTitre = h('p', { class: 'surtitre' });
  const corps = h('div', { class: 'panneau' });

  racine.append(
    h(
      'div',
      { class: 'reglages' },
      h(
        'aside',
        { class: 'cote' },
        h('header', { class: 'marque' }, h('span', { class: 'marque-mot', 'aria-label': 'réglages' }, 'réglages')),
        nav,
      ),
      h('div', { class: 'page' }, h('article', { class: 'feuille' }, h('header', { class: 'entete' }, sousTitre, titre), corps)),
    ),
  );


  let demonter: void | (() => void);
  let courant: string | null = null;

  function montrer(id?: string | null): void {
    const p = panneau(id) ?? PANNEAUX[0]!;
    // Les panneaux ajoutés après le montage apparaissent dans la liste.
    dessinerListe();
    for (const b of nav.children) b.setAttribute('aria-current', String(b.getAttribute('data-panneau') === p.id));
    if (p.id === courant) return;
    courant = p.id;
    if (typeof demonter === 'function') demonter();
    sousTitre.textContent = p.sousTitre;
    poserTitre(titre, p.titre);
    corps.dataset.panneau = p.id;
    // Un hôte neuf par panneau : un panneau lent (micros, stockage…) qui
    // répond après qu'on l'a quitté écrit dans un nœud détaché, pas dans le
    // suivant (KOB-232).
    const hote = h('div', { class: 'panneau-hote' });
    corps.replaceChildren(hote);
    demonter = p.monter(hote, { notifier });
  }

  function dessinerListe(): void {
    if (nav.children.length === PANNEAUX.length) return;
    nav.replaceChildren(
      ...PANNEAUX.map((p) =>
        h(
          'button',
          { type: 'button', class: 'menu-entree', 'data-panneau': p.id, onclick: () => montrer(p.id) },
          h('span', { class: 'menu-titre' }, p.titre),
          h('span', { class: 'menu-sous-titre' }, p.sousTitre),
        ),
      ),
    );
  }

  return { montrer };
}
