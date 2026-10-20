// Sections de l'app Kobi (KOB-159) qui n'ont pas de module à elles :
// Rappels et tâches, Activité (le journal), À propos. Le Carnet (Notes et réunions), les
// Réglages et l'accueil viennent de leurs propres modules.

import { carnet, type Rappel } from '../carnet/donnees';
import { cleDuJour, echeance, heure, libelleDuJour } from '../carnet/format';
import { animationsReduites, h } from '../dom';
import { etapeLigne } from '../activite';
import type { Bulle } from '../conversation';
import { surLangue, t } from '../i18n';
import { absente, ecouter, invoke, message } from '../ipc';
import type { Ton } from '../reglages/panneau';
import { poserTitre } from '../titre';
import { blocValidations } from './validations';
import { reagir } from './mini-kobi';

type Notifier = (texte: string, ton?: Ton) => void;

function entete(surtitre: string, titre: string, aide: string): HTMLElement {
  const h1 = h('h1', { class: 'titre' });
  poserTitre(h1, titre);
  return h('header', { class: 'entete' }, h('p', { class: 'surtitre' }, surtitre), h1, h('p', { class: 'aide' }, aide));
}

// ----- Rappels et tâches -----

export interface VueRappels {
  rafraichir(): Promise<void>;
}

/** Rappels (« Kobi, rappelle-moi … », KOB-145) : à venir d'abord, puis faits. */
export function monterRappels(racine: HTMLElement, notifier: Notifier): VueRappels {
  const liste = h('div', { class: 'rappels-liste' });
  const tete = () => entete(t('app.sections.rappels.surtitre'), t('app.sections.rappels.titre'), t('app.sections.rappels.aide'));
  let enTete = tete();
  racine.append(h('article', { class: 'feuille' }, enTete, liste));
  // Langue changée (KOB-208) : l'en-tête et la liste se redessinent.
  surLangue(() => {
    const nouvel = tete();
    enTete.replaceWith(nouvel);
    enTete = nouvel;
    void rafraichir();
  });

  const ligne = (r: Rappel, i: number) => {
    const e = echeance(r.echeance_ms);
    return h(
      'div',
      { class: 'rappel', 'data-id': r.id, 'data-fait': String(r.fait), 'data-retard': String(!r.fait && e.retard), style: `--i:${Math.min(i, 20)}` },
      h('button', {
        type: 'button',
        class: 'coche',
        'aria-label': r.fait ? t('app.sections.rappels.estFait', { texte: r.texte }) : t('app.sections.rappels.marquerFait', { texte: r.texte }),
        disabled: r.fait,
        onclick: async (ev: Event) => {
          const el = (ev.currentTarget as HTMLElement).parentElement!;
          try {
            await carnet.rappelFait(r.id);
            el.classList.add('fait');
            notifier(t('app.sections.rappels.rappelFait'));
            reagir('fait');
            window.setTimeout(() => void rafraichir(), animationsReduites() ? 0 : 450);
          } catch (err) {
            notifier(message(err), 'erreur');
          }
        },
      }),
      h('span', { class: 'rappel-corps' }, h('span', { class: 'rappel-texte' }, r.texte), h('span', { class: 'rappel-quand' }, r.fait ? t('app.sections.rappels.fait') : e.retard ? t('app.sections.rappels.enRetard', { quand: e.texte }) : e.texte)),
    );
  };

  async function rafraichir(): Promise<void> {
    let rappels: Rappel[];
    try {
      rappels = (await carnet.index()).rappels;
    } catch (e) {
      liste.replaceChildren(h('p', { class: 'erreur' }, message(e)));
      return;
    }
    const avenir = rappels.filter((r) => !r.fait);
    const faits = rappels.filter((r) => r.fait);
    liste.replaceChildren(
      h('h2', { class: 'groupe' }, t('app.sections.rappels.aVenir', { n: avenir.length })),
      ...(avenir.length ? avenir.map(ligne) : [h('p', { class: 'vide' }, t('app.sections.rappels.rienAVenir'))]),
      ...(faits.length ? [h('h2', { class: 'groupe' }, t('app.sections.rappels.faits', { n: faits.length })), ...faits.map(ligne)] : []),
    );
  }

  void rafraichir();
  return { rafraichir };
}

// ----- Activité -----

/** Entrée du journal (`journal_activite`, événement `journal`). */
export interface EntreeActivite {
  quand_ms: number;
  genre: 'question' | 'reunion' | 'recherche' | 'action' | 'validation' | 'erreur';
  texte: string;
  detail?: string | null;
}

/** Glyphe de chaque genre d'entrée ; son libellé est `app.sections.genre.<genre>`. */
const GENRES: Record<EntreeActivite['genre'], string> = {
  question: '?',
  reunion: '●',
  recherche: '⌕',
  action: '!',
  validation: '✓',
  erreur: 'x',
};

export interface VueActivite {
  rafraichir(): Promise<void>;
}

/**
 * Ce que Kobi a fait : en haut, chaque échange récent avec ses étapes
 * (fichiers lus, recherches, commandes, appels MCP ; KOB-152, événement
 * `conversation`) ; dessous, le journal daté des questions, réunions,
 * actions proposées et de tes réponses (`journal_activite`, événement
 * `journal`), le plus récent en haut, regroupé par jour.
 */
export function monterActivite(racine: HTMLElement): VueActivite {
  const etapes = h('div', { class: 'journal', role: 'log', 'aria-label': t('app.sections.activite.etapes') });
  const liste = h('div', { class: 'fil', 'aria-live': 'polite' });
  const tete = () => entete(t('app.sections.activite.surtitre'), t('app.sections.activite.titre'), t('app.sections.activite.aide'));
  let enTete = tete();
  racine.append(h('article', { class: 'feuille' }, enTete, blocValidations(), etapes, liste));
  // Langue changée (KOB-208) : l'en-tête et le journal se redessinent.
  surLangue(() => {
    const nouvel = tete();
    enTete.replaceWith(nouvel);
    enTete = nouvel;
    etapes.setAttribute('aria-label', t('app.sections.activite.etapes'));
    void rafraichir();
  });

  const afficherEtapes = (bulles: unknown) => {
    if (!Array.isArray(bulles)) return;
    const avec = (bulles as Bulle[]).filter((b) => (b.etapes ?? []).length > 0).reverse();
    etapes.hidden = avec.length === 0;
    etapes.replaceChildren(
      ...avec.map((b) =>
        h(
          'section',
          { class: 'journal-tour', 'data-tour': b.tour },
          h('p', { class: 'journal-question' }, b.question),
          h('div', { class: 'journal-etapes' }, ...(b.etapes ?? []).map((e) => etapeLigne(e))),
        ),
      ),
    );
  };
  invoke<Bulle[]>('historique').then(afficherEtapes, () => afficherEtapes([]));
  ecouter<Bulle[]>('conversation', afficherEtapes);

  let entrees: EntreeActivite[] = [];

  const ligne = (e: EntreeActivite, i: number) => {
    const glyphe = GENRES[e.genre] ?? '·';
    const libelle = GENRES[e.genre] ? t(`app.sections.genre.${e.genre}`) : '';
    return h(
      'div',
      { class: 'entree', 'data-genre': e.genre, style: `--i:${Math.min(i, 20)}` },
      h('span', { class: 'entree-glyphe', 'aria-hidden': 'true' }, glyphe),
      h(
        'span',
        { class: 'entree-corps' },
        h('span', { class: 'entree-texte' }, e.texte),
        h('span', { class: 'entree-meta' }, [heure(e.quand_ms), libelle, e.detail].filter(Boolean).join(' · ')),
      ),
    );
  };

  function dessiner(): void {
    if (!entrees.length) {
      liste.replaceChildren(
        h(
          'div',
          { class: 'bientot' },
          h('span', { class: 'bientot-yeux', 'aria-hidden': 'true' }, '- -'),
          h('p', {}, t('app.sections.activite.vide')),
        ),
      );
      return;
    }
    const enfants: HTMLElement[] = [];
    let jour = '';
    let i = 0;
    for (const e of [...entrees].reverse()) {
      const cle = cleDuJour(e.quand_ms);
      if (cle !== jour) {
        jour = cle;
        enfants.push(h('h2', { class: 'groupe' }, libelleDuJour(cle)));
      }
      enfants.push(ligne(e, i++));
    }
    liste.replaceChildren(...enfants);
  }

  async function rafraichir(): Promise<void> {
    try {
      entrees = await invoke<EntreeActivite[]>('journal_activite');
    } catch (e) {
      liste.replaceChildren(h('p', { class: 'erreur' }, absente(e) ? t('app.sections.activite.absent') : message(e)));
      return;
    }
    dessiner();
  }

  ecouter<EntreeActivite>('journal', (e) => {
    entrees.push(e);
    dessiner();
    liste.querySelector('.entree')?.classList.add('nouvelle');
  });
  void rafraichir();
  return { rafraichir };
}

// ----- À propos -----

interface APropos {
  version: string;
  dossier: string | null;
}

export function monterAPropos(racine: HTMLElement): void {
  const infos = h('dl', { class: 'infos' });
  const feuille = h('article', { class: 'feuille' });
  const dessiner = () =>
    feuille.replaceChildren(
      entete(t('app.sections.aPropos.surtitre'), 'Kobi', t('app.sections.aPropos.aide')),
      infos,
      h(
        'ul',
        { class: 'principes' },
        h('li', {}, t('app.sections.aPropos.audio')),
        h('li', {}, t('app.sections.aPropos.rienEnregistre')),
        h('li', {}, t('app.sections.aPropos.lecture')),
      ),
    );
  let a: APropos | null = null;
  const remplir = () => {
    if (!a)
      return infos.replaceChildren(h('dt', {}, t('app.sections.aPropos.version')), h('dd', {}, '—'));
    infos.replaceChildren(
      h('dt', {}, t('app.sections.aPropos.version')),
      h('dd', {}, a.version),
      h('dt', {}, t('app.sections.aPropos.dossier')),
      h('dd', {}, h('code', {}, a.dossier ?? '—')),
      h('dt', {}, t('app.sections.aPropos.licence')),
      h('dd', {}, t('app.sections.aPropos.openSource')),
    );
  };
  dessiner();
  racine.append(feuille);
  let charge = false;
  // Langue changée (KOB-208) : la page se redessine, sans redemander.
  surLangue(() => {
    dessiner();
    if (charge) remplir();
  });
  invoke<APropos>('a_propos').then(
    (x) => {
      a = x;
      charge = true;
      remplir();
    },
    () => {
      charge = true;
      remplir();
    },
  );
}
