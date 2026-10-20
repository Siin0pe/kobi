// Panneau « Stockage » (KOB-141, KOB-140) : dossier racine des réunions et
// conversations, durée de conservation, publication par MCP (montrée en
// lecture seule tant que Kobi ne fait que lire, KOB-138).

import { h } from '../dom';
import { t } from '../i18n';
import { invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

export type Retention = number | 'unlimited';

export type EtatPublication = { etat: 'aucune' } | { etat: 'reglee'; mcp: string; destination: string };

/** Réglages renvoyés par `reglages_stockage` et `regler_stockage`. */
export interface ReglagesStockage {
  dossier: string | null;
  par_defaut: boolean;
  retention_reunions: Retention;
  retention_conversations: Retention;
  publication_reunions: EtatPublication;
  publication_conversations: EtatPublication;
}

/** Changement envoyé à `regler_stockage` ; un champ absent reste tel quel. */
export interface ChangementStockage {
  dossier?: string;
  retention_reunions?: Retention;
  retention_conversations?: Retention;
}

/** Durées proposées, libellées au rendu (la langue change à chaud). */
export function durees(): ReadonlyArray<{ valeur: Retention; libelle: string }> {
  return [
    { valeur: 30, libelle: t('reglages.stockage.jours', { n: 30 }) },
    { valeur: 90, libelle: t('reglages.stockage.jours', { n: 90 }) },
    { valeur: 365, libelle: t('reglages.stockage.ans', { n: 1 }) },
    { valeur: 'unlimited', libelle: t('reglages.stockage.toujours') },
  ];
}

function regler(changement: ChangementStockage): Promise<ReglagesStockage> {
  return invoke<ReglagesStockage>('regler_stockage', { changement });
}

function choixDuree(
  nom: string,
  legende: string,
  aide: string,
  valeur: Retention,
  changer: (v: Retention) => void,
): HTMLElement {
  const groupe = h('div', { class: 'segments', role: 'radiogroup', 'aria-label': legende });
  for (const d of durees()) {
    const bouton = h(
      'button',
      {
        type: 'button',
        role: 'radio',
        class: 'segment',
        'data-valeur': String(d.valeur),
        'aria-checked': String(d.valeur === valeur),
        onclick: () => {
          if (bouton.getAttribute('aria-checked') === 'true') return;
          for (const b of groupe.children) b.setAttribute('aria-checked', String(b === bouton));
          changer(d.valeur);
        },
      },
      d.libelle,
    );
    groupe.append(bouton);
  }
  return h('div', { class: 'reglage', 'data-reglage': nom }, h('div', { class: 'reglage-titre' }, legende), h('p', { class: 'aide' }, aide), groupe);
}

/** `reunions` : le Carnet a un bouton « Publier » pour elles (KOB-75). */
function publication(titre: string, etat: EtatPublication, reunions: boolean): HTMLElement {
  const corps =
    etat.etat === 'aucune'
      ? h('span', {}, t('reglages.stockage.aucuneDestination'))
      : h(
          'span',
          {},
          h('b', {}, etat.mcp),
          ' → ',
          h('b', {}, etat.destination),
          h('br'),
          t(reunions ? 'reglages.stockage.publierReunions' : 'reglages.stockage.publierConversations'),
        );
  return h('li', { class: 'publication', 'data-etat': etat.etat }, h('span', { class: 'publication-titre' }, titre), corps);
}

/** Dessine le panneau à partir des réglages lus. */
function dessiner(el: HTMLElement, r: ReglagesStockage, ctx: Contexte, recharger: (r: ReglagesStockage) => void): void {
  const appliquer = async (changement: ChangementStockage, texte: string) => {
    try {
      recharger(await regler(changement));
      ctx.notifier(texte);
    } catch (e) {
      ctx.notifier(message(e), 'erreur');
      // Revenir à ce qui est vraiment enregistré.
      recharger(r);
    }
  };
  const choisir = async () => {
    try {
      const dossier = await invoke<string | null>('choisir_dossier');
      if (dossier) await appliquer({ dossier }, t('reglages.stockage.dossierChange'));
    } catch (e) {
      ctx.notifier(message(e), 'erreur');
    }
  };
  el.replaceChildren(
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'dossier' },
      h('div', { class: 'reglage-titre' }, t('reglages.stockage.dossierTitre')),
      h('p', { class: 'aide' }, t('reglages.stockage.dossierAide')),
      h(
        'div',
        { class: 'dossier' },
        h('code', { class: 'chemin', title: r.dossier ?? '' }, r.dossier ?? t('reglages.stockage.aucunDossier')),
        r.par_defaut && h('span', { class: 'puce' }, t('reglages.stockage.parDefaut')),
      ),
      h(
        'div',
        { class: 'boutons' },
        h('button', { type: 'button', class: 'bouton plein', 'data-action': 'choisir', onclick: () => void choisir() }, t('reglages.stockage.choisir')),
        h(
          'button',
          {
            type: 'button',
            class: 'bouton',
            'data-action': 'ouvrir',
            onclick: () => invoke('carnet_ouvrir_dossier').catch((e) => ctx.notifier(message(e), 'erreur')),
          },
          t('reglages.stockage.ouvrir'),
        ),
        !r.par_defaut &&
          h(
            'button',
            {
              type: 'button',
              class: 'bouton discret',
              'data-action': 'defaut',
              onclick: () => void appliquer({ dossier: '' }, t('reglages.stockage.retourDefaut')),
            },
            t('reglages.stockage.revenirDefaut'),
          ),
      ),
    ),
    choixDuree(
      'retention-reunions',
      t('reglages.stockage.garderReunions'),
      t('reglages.stockage.garderReunionsAide'),
      r.retention_reunions,
      (v) => void appliquer({ retention_reunions: v }, t('reglages.stockage.reunionsEnregistre')),
    ),
    choixDuree(
      'retention-conversations',
      t('reglages.stockage.garderConversations'),
      t('reglages.stockage.garderConversationsAide'),
      r.retention_conversations,
      (v) => void appliquer({ retention_conversations: v }, t('reglages.stockage.conversationsEnregistre')),
    ),
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'publication' },
      h('div', { class: 'reglage-titre' }, t('reglages.stockage.publicationTitre')),
      h('p', { class: 'aide' }, t('reglages.stockage.publicationAide')),
      h('ul', { class: 'publications' }, publication(t('reglages.stockage.reunions'), r.publication_reunions, true), publication(t('reglages.stockage.conversations'), r.publication_conversations, false)),
    ),
  );
}

export const STOCKAGE: Panneau = {
  id: 'stockage',
  get titre() {
    return t('reglages.stockage.titre');
  },
  get sousTitre() {
    return t('reglages.stockage.sousTitre');
  },
  monter(el, ctx) {
    el.replaceChildren(h('p', { class: 'chargement' }, t('reglages.stockage.chargement')));
    const recharger = (r: ReglagesStockage) => dessiner(el, r, ctx, recharger);
    invoke<ReglagesStockage>('reglages_stockage').then(recharger, (e) => {
      el.replaceChildren(h('p', { class: 'erreur' }, message(e)));
    });
  },
};
