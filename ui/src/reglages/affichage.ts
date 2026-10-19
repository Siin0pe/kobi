// Panneau « Affichage » (KOB-169) : widget simple ou bulle détaillée, vue
// agrandie de la conversation (ouverture automatique, taille, place, texte,
// délai). Chaque choix s'applique tout de suite dans la fenêtre de Kobi,
// sans redémarrer (affichage.ts).

import { affichage, changerAffichage, type Affichage } from '../affichage';
import { h } from '../dom';
import type { Panneau } from './panneau';

interface Groupe<K extends keyof Affichage> {
  cle: K;
  titre: string;
  aide: string;
  options: ReadonlyArray<{ valeur: Affichage[K]; titre: string; detail: string }>;
}

const GROUPES: [
  Groupe<'mode'>,
  Groupe<'auto'>,
  Groupe<'taille'>,
  Groupe<'position'>,
  Groupe<'texte'>,
  Groupe<'delai'>,
] = [
  {
    cle: 'mode',
    titre: 'Widget de Kobi',
    aide: 'Ce que Kobi montre à côté de lui sur le bureau.',
    options: [
      { valeur: 'simple', titre: 'Simple', detail: 'Kobi et une courte bulle de trois lignes au plus ; le reste se lit en grand.' },
      { valeur: 'detaille', titre: 'Bulle détaillée', detail: 'La bulle grandit avec la réponse et défile.' },
    ],
  },
  {
    cle: 'auto',
    titre: 'Ouvrir la conversation en grand',
    aide: 'On peut aussi l’ouvrir d’un clic sur la bulle, en disant « Kobi, montre en grand » ou avec Ctrl+Alt+G.',
    options: [
      { valeur: 'jamais', titre: 'À la demande', detail: 'Seulement quand je le demande : rien ne s’ouvre pendant que tu fais autre chose.' },
      { valeur: 'longues', titre: 'Pour les réponses longues', detail: 'Dès que la bulle ne peut plus tout montrer.' },
      { valeur: 'toujours', titre: 'Toujours', detail: 'À chaque réponse.' },
    ],
  },
  {
    cle: 'taille',
    titre: 'Taille de la vue',
    aide: 'La largeur et la hauteur du panneau de lecture.',
    options: [
      { valeur: 'petite', titre: 'Petite', detail: '480 × 420' },
      { valeur: 'moyenne', titre: 'Moyenne', detail: '640 × 560' },
      { valeur: 'grande', titre: 'Grande', detail: '860 × 720' },
    ],
  },
  {
    cle: 'position',
    titre: 'Place de la vue',
    aide: 'Kobi reste toujours visible à côté.',
    options: [
      { valeur: 'a-cote', titre: 'À côté de Kobi', detail: 'Du côté où il y a la place.' },
      { valeur: 'centre', titre: 'Au centre de l’écran', detail: 'Pour lire de loin.' },
      { valeur: 'moitie-droite', titre: 'Moitié droite de l’écran', detail: 'Toute la hauteur.' },
    ],
  },
  {
    cle: 'texte',
    titre: 'Taille du texte',
    aide: 'Dans la vue agrandie.',
    options: [
      { valeur: 'petit', titre: 'Petit', detail: '15 px' },
      { valeur: 'moyen', titre: 'Moyen', detail: '18 px' },
      { valeur: 'grand', titre: 'Grand', detail: '22 px' },
    ],
  },
  {
    cle: 'delai',
    titre: 'Ranger la vue',
    aide: 'Une fois la réponse finie ; Échap ou un clic à côté la rangent tout de suite.',
    options: [
      { valeur: 10, titre: 'Après 10 s', detail: '' },
      { valeur: 30, titre: 'Après 30 s', detail: '' },
      { valeur: 60, titre: 'Après 1 min', detail: '' },
      { valeur: 0, titre: 'Jamais toute seule', detail: '' },
    ],
  },
];

export const AFFICHAGE: Panneau = {
  id: 'affichage',
  titre: 'Affichage',
  sousTitre: 'Widget simple, conversation en grand',
  monter(el, ctx) {
    const blocs = GROUPES.map((g) => {
      const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': g.titre });
      const marquer = (valeur: unknown): void => {
        for (const b of choix.children) b.setAttribute('aria-checked', String(b.getAttribute('data-valeur') === String(valeur)));
      };
      for (const o of g.options as ReadonlyArray<{ valeur: string | number; titre: string; detail: string }>) {
        choix.append(
          h(
            'button',
            {
              type: 'button',
              role: 'radio',
              class: 'option',
              'data-valeur': String(o.valeur),
              onclick: () => {
                changerAffichage({ [g.cle]: o.valeur } as Partial<Affichage>);
                marquer(o.valeur);
                ctx.notifier(`${g.titre} : ${o.titre.toLowerCase()}`);
              },
            },
            h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
            h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, o.titre), o.detail ? h('span', { class: 'option-detail' }, o.detail) : ''),
          ),
        );
      }
      marquer(affichage()[g.cle]);
      return h('div', { class: 'reglage', 'data-reglage': g.cle }, h('div', { class: 'reglage-titre' }, g.titre), h('p', { class: 'aide' }, g.aide), choix);
    });
    el.replaceChildren(...blocs);
    ctx.signaler?.('ok', affichage().mode === 'simple' ? 'widget simple' : 'bulle détaillée');
  },
};
