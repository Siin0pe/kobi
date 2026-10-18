// Sections de l'app Kobi (KOB-159) qui n'ont pas de module à elles :
// Rappels et tâches, Activité (le journal), À propos. Le Carnet (Notes et réunions), les
// Réglages et l'accueil viennent de leurs propres modules.

import { carnet, type Rappel } from '../carnet/donnees';
import { cleDuJour, echeance, heure, libelleDuJour } from '../carnet/format';
import { animationsReduites, h } from '../dom';
import { etapeLigne } from '../activite';
import type { Bulle } from '../conversation';
import { absente, ecouter, invoke, message } from '../ipc';
import type { Ton } from '../reglages/panneau';
import { poserTitre } from '../titre';
import { blocValidations } from './validations';

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
  racine.append(
    h(
      'article',
      { class: 'feuille' },
      entete('Rappels et tâches', 'À ne pas oublier', 'Dis « Kobi, rappelle-moi … » : le rappel arrive ici et Kobi te le dit à l’heure.'),
      liste,
    ),
  );

  const ligne = (r: Rappel, i: number) => {
    const e = echeance(r.echeance_ms);
    return h(
      'div',
      { class: 'rappel', 'data-id': r.id, 'data-fait': String(r.fait), 'data-retard': String(!r.fait && e.retard), style: `--i:${Math.min(i, 20)}` },
      h('button', {
        type: 'button',
        class: 'coche',
        'aria-label': r.fait ? `« ${r.texte} » est fait` : `Marquer « ${r.texte} » comme fait`,
        disabled: r.fait,
        onclick: async (ev: Event) => {
          const el = (ev.currentTarget as HTMLElement).parentElement!;
          try {
            await carnet.rappelFait(r.id);
            el.classList.add('fait');
            notifier('Rappel fait.');
            window.setTimeout(() => void rafraichir(), animationsReduites() ? 0 : 450);
          } catch (err) {
            notifier(message(err), 'erreur');
          }
        },
      }),
      h('span', { class: 'rappel-corps' }, h('span', { class: 'rappel-texte' }, r.texte), h('span', { class: 'rappel-quand' }, r.fait ? 'Fait' : e.retard ? `En retard · ${e.texte}` : e.texte)),
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
      h('h2', { class: 'groupe' }, `À venir (${avenir.length})`),
      ...(avenir.length ? avenir.map(ligne) : [h('p', { class: 'vide' }, 'Rien à venir.')]),
      ...(faits.length ? [h('h2', { class: 'groupe' }, `Faits (${faits.length})`), ...faits.map(ligne)] : []),
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

/** Glyphe et libellé de chaque genre d'entrée. */
const GENRES: Record<EntreeActivite['genre'], [string, string]> = {
  question: ['?', 'Demandé à Kobi'],
  reunion: ['●', 'Réunion'],
  recherche: ['⌕', 'Recherche'],
  action: ['!', 'Action proposée'],
  validation: ['✓', 'Ta réponse'],
  erreur: ['x', 'Erreur'],
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
  const etapes = h('div', { class: 'journal', role: 'log', 'aria-label': 'Étapes des derniers échanges' });
  const liste = h('div', { class: 'fil', 'aria-live': 'polite' });
  racine.append(
    h(
      'article',
      { class: 'feuille' },
      entete('Activité', 'Ce que Kobi a fait', 'Tes questions et les étapes de son travail, les réunions, les actions proposées et tes réponses. Kobi ne fait rien d’important sans toi.'),
      blocValidations(),
      etapes,
      liste,
    ),
  );

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
    const [glyphe, libelle] = GENRES[e.genre] ?? ['·', ''];
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
          h('p', {}, 'Rien encore. Pose une question à Kobi ou lance une réunion : tout ce qu’il fait s’affiche ici.'),
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
      liste.replaceChildren(h('p', { class: 'erreur' }, absente(e) ? 'Le journal arrive avec la prochaine version de Kobi.' : message(e)));
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
  racine.append(
    h(
      'article',
      { class: 'feuille' },
      entete('À propos', 'Kobi', 'Assistant vocal de bureau, local et open source : il écoute sur cet ordinateur et répond avec ton Claude Code.'),
      infos,
      h(
        'ul',
        { class: 'principes' },
        h('li', {}, 'L’audio reste sur cet ordinateur : seul le texte de la conversation part chez Claude.'),
        h('li', {}, 'Rien n’est enregistré hors des réunions que tu lances.'),
        h('li', {}, 'Pour l’instant, Kobi ne fait que lire ton code.'),
      ),
    ),
  );
  invoke<APropos>('a_propos').then(
    (a) => {
      infos.replaceChildren(
        h('dt', {}, 'Version'),
        h('dd', {}, a.version),
        h('dt', {}, 'Dossier de Kobi'),
        h('dd', {}, h('code', {}, a.dossier ?? '—')),
        h('dt', {}, 'Licence'),
        h('dd', {}, 'Open source (voir le dépôt)'),
      );
    },
    () => infos.replaceChildren(h('dt', {}, 'Version'), h('dd', {}, '—')),
  );
}
