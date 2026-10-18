// L'app Kobi (KOB-159) : la seule fenêtre en plus de la mascotte. Navigation
// latérale et sections Notes et réunions (le Carnet), Rappels et tâches,
// Activité, Réglages, À propos ; écran d'accueil au premier démarrage (le
// premier lancement). Chaque section est montée à sa première visite puis
// gardée, masquée, pour retrouver son état.
//
// Adresse d'une section (ancre de l'URL, et charge de l'événement
// `app://section` envoyé par `app::ouvrir` côté Rust) : `notes`,
// `notes/<réunion>`, `rappels`, `activite`, `reglages`, `reglages/<panneau>`,
// `a-propos`, `accueil`.

import '../styles/fonts.css';
import '../reglages/reglages.css';
import './app.css';
import { monterAccueil } from '../bienvenue/main';
import { monterCarnet, type VueCarnet } from '../carnet/main';
import { animationsReduites, h } from '../dom';
import { disponible, ecouter } from '../ipc';
import { Blinker, Scrambler } from '../mascotte/glyphs';
import { creerNotifieur } from '../notif';
import { ajouterPanneau } from '../reglages/panneaux';
import { monterReglages, type VueReglages } from '../reglages/vue';
import { monterActivite, monterAPropos, monterRappels, type VueRappels } from './sections';

export type IdSection = 'notes' | 'rappels' | 'activite' | 'reglages' | 'a-propos';

interface Entree {
  id: IdSection;
  titre: string;
  /** Glyphe de la navigation (Recursive). */
  glyphe: string;
  /** Mot de Kobi quand on arrive sur la section. */
  mot: string;
}

/** Sections de la navigation, dans l'ordre. */
export const SECTIONS: readonly Entree[] = [
  { id: 'notes', titre: 'Notes et réunions', glyphe: '✎', mot: 'notes' },
  { id: 'rappels', titre: 'Rappels et tâches', glyphe: '◷', mot: 'rappels' },
  { id: 'activite', titre: 'Activité', glyphe: '≡', mot: 'journal' },
  { id: 'reglages', titre: 'Réglages', glyphe: '⚙', mot: 'réglages' },
  { id: 'a-propos', titre: 'À propos', glyphe: '?', mot: 'kobi' },
];

const notifier = creerNotifieur(document.body);

// ----- Squelette -----

const yeux = [h('span', { class: 'g' }, '*'), h('span', { class: 'g' }, '*')];
const mot = h('span', { class: 'mot', 'aria-hidden': 'true' });
const nav = h('nav', { class: 'nav-sections', 'aria-label': 'Sections de Kobi' });
const contenu = h('main', { class: 'contenu' });
const accueil = h('div', { class: 'vue vue-accueil', hidden: true });
const app = h(
  'div',
  { class: 'app', 'data-accueil': 'false' },
  h(
    'aside',
    { class: 'nav' },
    h('div', { class: 'nav-kobi', 'aria-hidden': 'true' }, h('span', { class: 'nav-yeux' }, ...yeux), mot),
    nav,
  ),
  contenu,
  accueil,
);
document.getElementById('app')!.append(app);

const brouilleur = new Scrambler(yeux, mot);
brouilleur.setInstant(animationsReduites());
const blinker = new Blinker((ferme) => brouilleur.blink(ferme));
blinker.run(!animationsReduites() && !document.hidden);
document.addEventListener('visibilitychange', () => blinker.run(!animationsReduites() && !document.hidden));

const vues = new Map<IdSection, HTMLElement>();
for (const s of SECTIONS) {
  nav.append(
    h(
      'button',
      { type: 'button', class: 'nav-entree', 'data-section': s.id, onclick: () => aller(s.id) },
      h('span', { class: 'nav-glyphe', 'aria-hidden': 'true' }, s.glyphe),
      h('span', { class: 'nav-titre' }, s.titre),
    ),
  );
  const vue = h('section', { class: `vue vue-${s.id}`, 'data-section': s.id, 'aria-label': s.titre, hidden: true });
  vues.set(s.id, vue);
  contenu.append(vue);
}

// ----- Sections, montées à la première visite -----

let carnet: VueCarnet | null = null;
let reglages: VueReglages | null = null;
let rappels: VueRappels | null = null;

function monter(id: IdSection): void {
  const vue = vues.get(id)!;
  if (vue.childElementCount) return;
  switch (id) {
    case 'notes':
      carnet = monterCarnet(vue, { reglages: (panneau) => aller(`reglages/${panneau}`) });
      break;
    case 'rappels':
      rappels = monterRappels(vue, notifier);
      break;
    case 'activite':
      monterActivite(vue);
      break;
    case 'reglages':
      reglages = monterReglages(vue, notifier);
      break;
    case 'a-propos':
      monterAPropos(vue);
      break;
  }
}

let courante: IdSection | null = null;

/** Va à une adresse de section (voir l'en-tête). */
export function aller(adresse: string): void {
  const [tete, ...reste] = adresse.replace(/^#/, '').split('/');
  const detail = reste.join('/') || null;
  if (tete === 'accueil') return montrerAccueil();
  const id = (SECTIONS.find((s) => s.id === tete)?.id ?? 'notes') as IdSection;
  cacherAccueil();
  monter(id);
  if (id !== courante) {
    for (const [cle, vue] of vues) vue.hidden = cle !== id;
    for (const b of nav.children) {
      if (b.getAttribute('data-section') === id) b.setAttribute('aria-current', 'page');
      else b.removeAttribute('aria-current');
    }
    const s = SECTIONS.find((x) => x.id === id)!;
    brouilleur.show('*', s.mot);
    document.title = `Kobi · ${s.titre}`;
    courante = id;
    // Retour sur une section : ses données ont pu changer.
    if (id === 'notes') void carnet?.rafraichir();
    if (id === 'rappels') void rappels?.rafraichir();
  }
  if (id === 'reglages') reglages?.montrer(detail);
  if (id === 'notes' && detail) void carnet?.ouvrirReunion(detail);
  const ancre = `#${detail ? `${id}/${detail}` : id}`;
  if (location.hash !== ancre) history.replaceState(null, '', ancre);
}

// ----- Accueil (premier lancement) -----

function montrerAccueil(): void {
  // Rejoué depuis les Réglages : on repart du début.
  accueil.replaceChildren();
  monterAccueil(accueil, { fini: () => aller('notes'), notifier });
  accueil.hidden = false;
  app.dataset.accueil = 'true';
  document.title = 'Bienvenue — Kobi';
  if (location.hash !== '#accueil') history.replaceState(null, '', '#accueil');
}

function cacherAccueil(): void {
  if (accueil.hidden) return;
  accueil.hidden = true;
  accueil.replaceChildren();
  app.dataset.accueil = 'false';
}

// ----- Ouverture -----

ecouter<string>('app://section', (adresse) => aller(adresse));
window.addEventListener('hashchange', () => aller(location.hash));

/** Accès pour les tests. */
declare global {
  interface Window {
    kobiApp: { aller(adresse: string): void; ajouterPanneau: typeof ajouterPanneau };
  }
}
window.kobiApp = { aller, ajouterPanneau };

aller(location.hash || 'notes');
if (!disponible()) notifier('Ouvrez l’app depuis Kobi.', 'erreur');
