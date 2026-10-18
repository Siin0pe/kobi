// Sections de l'app Kobi (KOB-159) qui n'ont pas de module à elles :
// Rappels et tâches, Activité, À propos. Le Carnet (Notes et réunions), les
// Réglages et l'accueil viennent de leurs propres modules.

import { carnet, type Rappel } from '../carnet/donnees';
import { echeance } from '../carnet/format';
import { animationsReduites, h } from '../dom';
import { etapeLigne } from '../activite';
import type { Bulle } from '../conversation';
import { ecouter, invoke, message } from '../ipc';
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

/**
 * Ce que Kobi a fait (KOB-152) : chaque échange récent avec ses étapes
 * (fichiers lus, recherches, commandes, appels MCP), tenu à jour en direct
 * par l'événement `conversation`. Les validations viendront avec KOB-156.
 */
export function monterActivite(racine: HTMLElement): void {
  const journal = h('div', { class: 'journal', role: 'log', 'aria-label': 'Étapes des derniers échanges' });
  racine.append(
    h(
      'article',
      { class: 'feuille' },
      entete('Activité', 'Ce que Kobi a fait', 'Le journal des recherches, des actions proposées et de tes validations.'),
      blocValidations(),
      journal,
      h(
        'div',
        { class: 'bientot' },
        h('span', { class: 'bientot-yeux', 'aria-hidden': 'true' }, '- -'),
        h('p', {}, 'Pour l’instant, Kobi ne fait que lire : rien n’est écrit ni envoyé sans toi. Les actions que tu valides apparaîtront ici.'),
      ),
    ),
  );
  const afficher = (bulles: unknown) => {
    if (!Array.isArray(bulles)) return;
    const avec = (bulles as Bulle[]).filter((b) => (b.etapes ?? []).length > 0).reverse();
    journal.replaceChildren(
      ...(avec.length === 0
        ? [h('p', { class: 'journal-vide' }, 'Aucune étape pour l’instant : pose une question sur ton code, chaque fichier lu et chaque recherche s’afficheront ici.')]
        : avec.map((b) =>
            h(
              'section',
              { class: 'journal-tour', 'data-tour': b.tour },
              h('p', { class: 'journal-question' }, b.question),
              h('div', { class: 'journal-etapes' }, ...(b.etapes ?? []).map((e) => etapeLigne(e))),
            ),
          )),
    );
  };
  invoke<Bulle[]>('historique').then(afficher, () => afficher([]));
  ecouter<Bulle[]>('conversation', afficher);
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
