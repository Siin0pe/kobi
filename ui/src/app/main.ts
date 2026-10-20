// L'app Kobi (KOB-159) : la seule fenêtre en plus de la mascotte. Navigation
// latérale et sections Notes et réunions (le Carnet), Rappels et tâches,
// Activité, Réglages, À propos, et En direct pendant une réunion (les notes
// de KOB-73) ; écran d'accueil au premier démarrage (le premier lancement).
// Chaque section est montée à sa première visite puis gardée, masquée, pour
// retrouver son état. Pendant une réunion, le bandeau REC reste en haut de
// toutes les sections ; le petit Kobi de la navigation suit la mascotte.
//
// Adresse d'une section (ancre de l'URL, et charge de l'événement
// `app://section` envoyé par `app::ouvrir` côté Rust) : `notes`,
// `notes/<réunion>`, `rappels`, `activite`, `reglages`, `reglages/<panneau>`,
// `a-propos`, `direct`, `accueil`.

import '../erreurs';
import '../styles/fonts.css';
import '../reglages/reglages.css';
import './app.css';
import { monterAccueil } from '../bienvenue/main';
import { suivreCadence } from '../cadence';
import { monterCarnet, type VueCarnet } from '../carnet/main';
import { animationsReduites, h } from '../dom';
import { Metronome } from '../econome';
import { disponible, ecouter, invoke } from '../ipc';
import { creerNotifieur } from '../notif';
import { initLangue, surLangue, t } from '../i18n';
import { vivreTitres } from '../titre';
import { monterNotesEnDirect, type VueDirect } from '../notes/direct';
import { monterTranscriptionBrute, type VueBrute } from '../notes/brut';
import { ajouterPanneau } from '../reglages/panneaux';
import { monterReglages, type VueReglages } from '../reglages/vue';
import { monterMiniKobi, reagir } from './mini-kobi';
import { monterBandeauRec } from './rec';
import { monterActivite, monterAPropos, monterRappels, type VueActivite, type VueRappels } from './sections';

export type IdSection = 'direct' | 'notes' | 'rappels' | 'activite' | 'reglages' | 'a-propos';

interface Entree {
  id: IdSection;
  /** Lu au rendu : la langue change à chaud (KOB-208). */
  readonly titre: string;
  /** Glyphe de la navigation (Recursive). */
  glyphe: string;
  /** Mot de Kobi quand on arrive sur la section (lu au rendu, lui aussi). */
  readonly mot: string;
  /** Dans la navigation seulement pendant une réunion. */
  reunion?: boolean;
}

/** Entrée de la navigation : titre `app.main.section.<id>`, mot `app.main.mot.<id>`. */
function entree(id: IdSection, glyphe: string, reunion = false): Entree {
  const cle = id === 'a-propos' ? 'aPropos' : id;
  return {
    id,
    glyphe,
    ...(reunion ? { reunion } : {}),
    get titre() {
      return t(`app.main.section.${cle}`);
    },
    get mot() {
      return t(`app.main.mot.${cle}`);
    },
  };
}

/** Sections de la navigation, dans l'ordre. */
export const SECTIONS: readonly Entree[] = [
  entree('direct', '●', true),
  entree('notes', '✎'),
  entree('rappels', '◷'),
  entree('activite', '≡'),
  entree('reglages', '⚙'),
  entree('a-propos', '?'),
];

const notifierBase = creerNotifieur(document.body);
/** Notification de l'app : le petit Kobi réagit aussi (noté, ou oups). */
const notifier: typeof notifierBase = (texte, ton, plus) => {
  notifierBase(texte, ton, plus);
  reagir(ton === 'erreur' ? 'erreur' : 'enregistre');
};

// ----- Squelette -----

const yeux = [h('span', { class: 'g' }, '*'), h('span', { class: 'g' }, '*')];
const mot = h('span', { class: 'mot', 'aria-hidden': 'true' });
const nav = h('nav', { class: 'nav-sections', 'aria-label': t('app.main.nav') });
const contenu = h('main', { class: 'contenu' });
const bandeau = monterBandeauRec(
  () => aller('direct'),
  (r) => {
    nav.querySelector<HTMLElement>('[data-section="direct"]')!.hidden = !r && courante !== 'direct';
    // Le petit Kobi de la mascotte sait déjà qu'elle note ; ici, la nav le montre.
    app.dataset.rec = String(!!r);
  },
);
// Les sections s'empilent : celle qui part joue sa sortie sous celle qui arrive.
const pile = h('div', { class: 'pile' });
contenu.append(bandeau.element, pile);
const accueil = h('div', { class: 'vue vue-accueil', hidden: true });
const app = h(
  'div',
  { class: 'app', 'data-accueil': 'false', 'data-rec': 'false' },
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

const miniKobi = monterMiniKobi(app.querySelector<HTMLElement>('.nav-kobi')!, yeux, mot);
// Fenêtre masquée : les animations CSS s'arrêtent aussi, et celles posées
// en JavaScript (lettres du brouilleur) se mettent en pause.
let enPause: Animation[] = [];
const masquer = () => {
  document.documentElement.classList.toggle('app-masquee', document.hidden);
  if (document.hidden) {
    enPause = document.getAnimations().filter((a) => a.playState === 'running' && !(a instanceof CSSAnimation));
    enPause.forEach((a) => a.pause());
  } else {
    enPause.forEach((a) => a.play());
    enPause = [];
  }
};
// Images par seconde réglées (KOB-200) : toute l'app suit le même métronome
// que la mascotte, appliqué à chaud.
const metronome = new Metronome(document.body, (fps) => (document.documentElement.dataset.fps = String(fps)));
document.documentElement.dataset.fps = '0';
suivreCadence((cadence, voulue) => {
  document.documentElement.dataset.cadence = String(voulue);
  metronome.setCadence(cadence);
});
document.addEventListener('visibilitychange', () => metronome.setSuspended(document.hidden));
document.addEventListener('visibilitychange', masquer);
masquer();
// Titres vivants : une lettre d'un titre visible se brouille de temps en temps.
vivreTitres(contenu, '.titre, .marque-mot');
// Langue de l'interface (KOB-207) : celle de la config, suivie à chaud.
void initLangue();

const vues = new Map<IdSection, HTMLElement>();
for (const s of SECTIONS) {
  nav.append(
    h(
      'button',
      { type: 'button', class: 'nav-entree', 'data-section': s.id, hidden: !!s.reunion, onclick: () => aller(s.id) },
      h('span', { class: 'nav-glyphe', 'aria-hidden': 'true' }, s.glyphe),
      h('span', { class: 'nav-titre' }, s.titre),
    ),
  );
  const vue = h('section', { class: `vue vue-${s.id}`, 'data-section': s.id, 'aria-label': s.titre, hidden: true });
  vues.set(s.id, vue);
  pile.append(vue);
}

// Langue changée (KOB-208) : la navigation, les noms des sections, le titre
// de la fenêtre et le mot du petit Kobi se redisent.
surLangue(() => {
  nav.setAttribute('aria-label', t('app.main.nav'));
  for (const s of SECTIONS) {
    const b = nav.querySelector(`[data-section="${s.id}"] .nav-titre`);
    if (b) b.textContent = s.titre;
    vues.get(s.id)?.setAttribute('aria-label', s.titre);
  }
  if (!accueil.hidden) document.title = t('app.main.titreAccueil');
  const s = SECTIONS.find((x) => x.id === courante);
  if (s) {
    miniKobi.section(s.mot);
    if (accueil.hidden) document.title = `Kobi · ${s.titre}`;
  }
});

// ----- Sections, montées à la première visite -----

let carnet: VueCarnet | null = null;
let reglages: VueReglages | null = null;
let rappels: VueRappels | null = null;
let direct: VueDirect | null = null;
let brut: VueBrute | null = null;
let activite: VueActivite | null = null;

function monter(id: IdSection): void {
  const vue = vues.get(id)!;
  if (vue.childElementCount) return;
  switch (id) {
    case 'direct':
      direct = monterNotesEnDirect(vue);
      brut = monterTranscriptionBrute(vue);
      break;
    case 'notes':
      carnet = monterCarnet(vue, { reglages: (panneau) => aller(`reglages/${panneau}`) });
      break;
    case 'rappels':
      rappels = monterRappels(vue, notifier);
      break;
    case 'activite':
      activite = monterActivite(vue);
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
    transition(courante, id);
    for (const b of nav.children) {
      if (b.getAttribute('data-section') === id) b.setAttribute('aria-current', 'page');
      else b.removeAttribute('aria-current');
    }
    const s = SECTIONS.find((x) => x.id === id)!;
    miniKobi.section(s.mot);
    direct?.montrer(id === 'direct');
    brut?.montrer(id === 'direct');
    // Hors réunion, l'entrée « En direct » n'est là que si on y est.
    nav.querySelector<HTMLElement>('[data-section="direct"]')!.hidden = !bandeau.courant() && id !== 'direct';
    bandeau.element.dataset.ici = String(id === 'direct');
    document.title = `Kobi · ${s.titre}`;
    courante = id;
    // Retour sur une section : ses données ont pu changer.
    if (id === 'notes') void carnet?.rafraichir();
    if (id === 'rappels') void rappels?.rafraichir();
    if (id === 'activite') void activite?.rafraichir();
  }
  if (id === 'reglages') reglages?.montrer(detail);
  if (id === 'notes' && detail) void carnet?.ouvrirReunion(detail);
  const ancre = `#${detail ? `${id}/${detail}` : id}`;
  if (location.hash !== ancre) history.replaceState(null, '', ancre);
}

/**
 * Change de section : la nouvelle entre du côté où elle est dans la
 * navigation (d'en bas si elle est plus bas), l'ancienne s'efface en
 * glissant de l'autre côté. Elle est déjà `hidden` pendant sa sortie (pour
 * les lecteurs d'écran et les clics) ; `.sort` la garde dessinée.
 */
function transition(de: IdSection | null, vers: IdSection): void {
  const rang = (x: IdSection | null) => SECTIONS.findIndex((s) => s.id === x);
  const sens = rang(vers) >= rang(de) ? 'bas' : 'haut';
  for (const [cle, vue] of vues) {
    vue.hidden = cle !== vers;
    if (cle === vers) {
      vue.classList.remove('sort');
      vue.dataset.sens = sens;
    } else if (cle === de && !animationsReduites()) {
      vue.dataset.sens = sens;
      vue.classList.add('sort');
      const fin = (e: AnimationEvent) => {
        // Les animations des enfants remontent aussi : seule la sortie compte.
        if (e.target !== vue) return;
        vue.classList.remove('sort');
        vue.removeEventListener('animationend', fin);
      };
      vue.addEventListener('animationend', fin);
    }
  }
}

// ----- Accueil (premier lancement) -----

function montrerAccueil(): void {
  // Rejoué depuis les Réglages : on repart du début.
  accueil.replaceChildren();
  monterAccueil(accueil, { fini: () => aller('notes'), notifier });
  accueil.hidden = false;
  app.dataset.accueil = 'true';
  document.title = t('app.main.titreAccueil');
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

// La liste du Carnet suit la réunion (KOB-187) : son début, sa fin et le
// compte rendu changent l'entrée (badge REC, durée) sans attendre un retour
// sur la fenêtre.
let enregistre: boolean | null = null;
ecouter<{ actif?: boolean } | null>('enregistrement', (r) => {
  const actif = Boolean(r?.actif);
  if (actif === enregistre) return;
  enregistre = actif;
  void carnet?.rafraichir();
});
ecouter('compte_rendu', () => void carnet?.rafraichir());
window.addEventListener('hashchange', () => aller(location.hash));

/** Accès pour les tests. */
declare global {
  interface Window {
    kobiApp: { aller(adresse: string): void; ajouterPanneau: typeof ajouterPanneau };
  }
}
window.kobiApp = { aller, ajouterPanneau };

aller(location.hash || 'notes');
// Une section demandée pendant le chargement de la page (KOB-189) : son
// événement est parti avant qu'on l'écoute. La dernière demande gagne.
invoke<string | null>('app_section_demandee').then(
  (adresse) => {
    if (adresse && adresse !== location.hash.replace(/^#/, '')) aller(adresse);
  },
  () => {},
);
if (!disponible()) notifier(t('app.main.horsKobi'), 'erreur');
// Serveurs MCP importés de Claude Code sans choix, désactivés au démarrage
// (KOB-239) : l'app le dit une fois, discrètement.
invoke<boolean>('mcp_choix_a_proposer').then(
  (proposer) => {
    if (proposer) notifier(t('app.main.mcpAChoisir'));
  },
  () => {},
);
