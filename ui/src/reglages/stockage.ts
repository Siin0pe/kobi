// Panneau « Stockage » (KOB-141, KOB-140) : dossier racine des réunions et
// conversations, durée de conservation, publication par MCP (montrée en
// lecture seule tant que Kobi ne fait que lire, KOB-138).

import { h } from '../dom';
import { invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

export type Retention = number | 'unlimited';

export type EtatPublication = { etat: 'aucune' } | { etat: 'lecture-seule'; mcp: string; destination: string };

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

export const DUREES: ReadonlyArray<{ valeur: Retention; libelle: string }> = [
  { valeur: 30, libelle: '30 jours' },
  { valeur: 90, libelle: '90 jours' },
  { valeur: 365, libelle: '1 an' },
  { valeur: 'unlimited', libelle: 'Toujours' },
];

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
  for (const d of DUREES) {
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

function publication(titre: string, etat: EtatPublication): HTMLElement {
  const corps =
    etat.etat === 'aucune'
      ? h('span', {}, 'Aucune destination : tout reste sur cet ordinateur.')
      : h(
          'span',
          {},
          h('b', {}, etat.mcp),
          ' → ',
          h('b', {}, etat.destination),
          h('span', { class: 'puce' }, 'en attente'),
          h('br'),
          'Kobi ne fait que lire pour l’instant : rien n’est envoyé.',
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
      if (dossier) await appliquer({ dossier }, 'Dossier changé : la prochaine réunion y sera rangée.');
    } catch (e) {
      ctx.notifier(message(e), 'erreur');
    }
  };
  el.replaceChildren(
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'dossier' },
      h('div', { class: 'reglage-titre' }, 'Dossier de Kobi'),
      h('p', { class: 'aide' }, 'Réunions, conversations et rappels sont rangés ici, en fichiers lisibles. Un changement vaut pour la prochaine réunion.'),
      h(
        'div',
        { class: 'dossier' },
        h('code', { class: 'chemin', title: r.dossier ?? '' }, r.dossier ?? 'aucun dossier personnel'),
        r.par_defaut && h('span', { class: 'puce' }, 'par défaut'),
      ),
      h(
        'div',
        { class: 'boutons' },
        h('button', { type: 'button', class: 'bouton plein', 'data-action': 'choisir', onclick: () => void choisir() }, 'Choisir un dossier…'),
        h(
          'button',
          {
            type: 'button',
            class: 'bouton',
            'data-action': 'ouvrir',
            onclick: () => invoke('carnet_ouvrir_dossier').catch((e) => ctx.notifier(message(e), 'erreur')),
          },
          'Ouvrir',
        ),
        !r.par_defaut &&
          h(
            'button',
            {
              type: 'button',
              class: 'bouton discret',
              'data-action': 'defaut',
              onclick: () => void appliquer({ dossier: '' }, 'Retour au dossier par défaut.'),
            },
            'Revenir au dossier par défaut',
          ),
      ),
    ),
    choixDuree(
      'retention-reunions',
      'Garder les réunions',
      'Au-delà, transcription, notes et compte rendu sont supprimés au démarrage de Kobi.',
      r.retention_reunions,
      (v) => void appliquer({ retention_reunions: v }, 'Conservation des réunions enregistrée.'),
    ),
    choixDuree(
      'retention-conversations',
      'Garder les conversations',
      'Les échanges du quotidien avec Kobi, rangés à part des réunions.',
      r.retention_conversations,
      (v) => void appliquer({ retention_conversations: v }, 'Conservation des conversations enregistrée.'),
    ),
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'publication' },
      h('div', { class: 'reglage-titre' }, 'Publication par MCP'),
      h('p', { class: 'aide' }, 'Copie d’un compte rendu vers un outil (Linear, Notion…). Prête, active quand Kobi pourra écrire.'),
      h('ul', { class: 'publications' }, publication('Réunions', r.publication_reunions), publication('Conversations', r.publication_conversations)),
    ),
  );
}

export const STOCKAGE: Panneau = {
  id: 'stockage',
  titre: 'Stockage',
  sousTitre: 'Dossier, conservation, publication',
  monter(el, ctx) {
    el.replaceChildren(h('p', { class: 'chargement' }, 'Lecture des réglages…'));
    const recharger = (r: ReglagesStockage) => dessiner(el, r, ctx, recharger);
    invoke<ReglagesStockage>('reglages_stockage').then(recharger, (e) => {
      el.replaceChildren(h('p', { class: 'erreur' }, message(e)));
    });
  },
};
