// Fenêtre Réglages : la liste des panneaux à gauche, le panneau choisi à
// droite. Ouverte par `ouvrir_reglages` (menu du clic droit, barre système),
// avec `?panneau=<id>` ; l'événement `reglages://panneau` change de panneau
// quand la fenêtre est déjà ouverte.

import '../styles/fonts.css';
import './reglages.css';
import './fenetre.css';
import { animationsReduites, h } from '../dom';
import { disponible, ecouter } from '../ipc';
import { Blinker } from '../mascotte/glyphs';
import { creerNotifieur } from '../notif';
import { poserTitre } from '../titre';
import { PANNEAUX, panneau } from './panneaux';

const notifier = creerNotifieur(document.body);
const nav = h('nav', { class: 'menu', 'aria-label': 'Réglages' });
const titre = h('h1', { class: 'titre' });
const sousTitre = h('p', { class: 'surtitre' });
const corps = h('div', { class: 'panneau' });
const yeux = [h('span', { class: 'oeil' }, 'o'), h('span', { class: 'oeil' }, 'o')];

document.getElementById('app')!.append(
  h(
    'div',
    { class: 'reglages' },
    h(
      'aside',
      { class: 'cote' },
      h('header', { class: 'marque' }, h('span', { class: 'mini-kobi', 'aria-hidden': 'true' }, ...yeux), h('span', { class: 'marque-mot' }, 'réglages')),
      nav,
    ),
    h('main', { class: 'page' }, h('article', { class: 'feuille' }, h('header', { class: 'entete' }, sousTitre, titre), corps)),
  ),
);

const blinker = new Blinker((ferme) => yeux.forEach((o) => (o.textContent = ferme ? '–' : 'o')));
blinker.run(!animationsReduites());

let demonter: void | (() => void);

function montrer(id: string | null | undefined): void {
  const p = panneau(id) ?? PANNEAUX[0]!;
  for (const b of nav.children) b.setAttribute('aria-current', String(b.getAttribute('data-panneau') === p.id));
  if (typeof demonter === 'function') demonter();
  sousTitre.textContent = p.sousTitre;
  poserTitre(titre, p.titre);
  corps.dataset.panneau = p.id;
  demonter = p.monter(corps, { notifier });
  history.replaceState(null, '', `?panneau=${p.id}`);
}

for (const p of PANNEAUX) {
  nav.append(
    h(
      'button',
      { type: 'button', class: 'menu-entree', 'data-panneau': p.id, onclick: () => montrer(p.id) },
      h('span', { class: 'menu-titre' }, p.titre),
      h('span', { class: 'menu-sous-titre' }, p.sousTitre),
    ),
  );
}

ecouter<string | null>('reglages://panneau', (id) => montrer(id));
montrer(new URLSearchParams(location.search).get('panneau'));
if (!disponible()) notifier('Ouvrez les réglages depuis Kobi.', 'erreur');
