// Panneau « Son de l'ordinateur » (KOB-164) : en réunion, Kobi peut écouter
// aussi la sortie audio de l'ordinateur (la voix des autres en visio), sur
// une piste « eux » à côté du micro. Désactivé par défaut ; le changement
// vaut tout de suite, même en pleine réunion. Monté dans la section Réglages
// de l'app et proposé, décoché, au premier lancement.

import { h } from '../dom';
import { absente, invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

/** Ce que renvoient `son_ordi` et `regler_son_ordi`. */
export interface EtatSonOrdi {
  actif: boolean;
  /** Sortie choisie ; `null` : celle du système. */
  sortie: string | null;
  /** La piste « eux » est ouverte (réunion en cours). */
  capte: boolean;
  /** `nom` est enregistré, `libelle` affiché (sous Linux, la description du sink). */
  sorties: Array<{ nom: string; libelle: string }>;
  defaut: string | null;
}

const EXPLICATION =
  'En réunion, Kobi transcrit aussi ce que joue l’ordinateur : la voix des autres participants en visio, mais aussi une vidéo ou une musique. Tout reste sur cet ordinateur. Hors réunion, seul le micro compte : le son de l’ordinateur ne réveille jamais Kobi.';

function dessiner(el: HTMLElement, e: EtatSonOrdi, ctx: Contexte, regler: (actif: boolean, sortie: string | null) => void): void {
  const interrupteur = h(
    'button',
    {
      type: 'button',
      role: 'switch',
      class: 'interrupteur',
      'aria-checked': String(e.actif),
      'aria-label': 'Écouter aussi le son de l’ordinateur',
      onclick: () => regler(!e.actif, e.sortie),
    },
    h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
  );
  const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': 'Sortie audio écoutée' });
  const options: Array<{ nom: string | null; libelle: string; detail?: string }> = [
    { nom: null, libelle: 'Celle du système', detail: e.defaut ?? 'sortie par défaut' },
    ...e.sorties.map((s) => ({ nom: s.nom, libelle: s.libelle })),
  ];
  for (const o of options) {
    choix.append(
      h(
        'button',
        {
          type: 'button',
          role: 'radio',
          class: 'option',
          'data-nom': o.nom ?? '',
          'aria-checked': String(o.nom === e.sortie),
          onclick: () => regler(e.actif, o.nom),
        },
        h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
        h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, o.libelle), o.detail && h('span', { class: 'option-detail' }, o.detail)),
      ),
    );
  }
  el.replaceChildren(
    h('p', { class: 'intro' }, EXPLICATION),
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'son-ordi' },
      h('div', { class: 'reglage-titre' }, 'Écouter aussi le son de l’ordinateur'),
      h('p', { class: 'aide' }, 'Pour les réunions en visio. Désactivé, Kobi n’ouvre jamais la sortie audio.'),
      h(
        'div',
        { class: 'ligne-interrupteur' },
        interrupteur,
        h('span', {}, e.actif ? (e.capte ? 'Activé · capté en ce moment' : 'Activé, en réunion') : 'Désactivé'),
      ),
    ),
  );
  if (e.actif) {
    el.append(
      h(
        'div',
        { class: 'reglage', 'data-reglage': 'sortie' },
        h('div', { class: 'reglage-titre' }, 'Sortie écoutée'),
        h('p', { class: 'aide' }, 'Celle où tu entends la visio (casque, haut-parleurs).'),
        choix,
      ),
    );
  }
  ctx.signaler?.(e.actif ? 'ok' : 'attente', e.actif ? 'activé' : 'désactivé');
}

export const SON_ORDI: Panneau = {
  id: 'son-ordi',
  titre: 'Son de l’ordinateur',
  sousTitre: 'La voix des autres en visio',
  monter(el, ctx) {
    const charger = (e: EtatSonOrdi) => dessiner(el, e, ctx, regler);
    const regler = async (actif: boolean, sortie: string | null) => {
      try {
        const e = await invoke<EtatSonOrdi>('regler_son_ordi', { actif, sortie });
        charger(e);
        ctx.notifier(e.actif ? 'Kobi écoutera aussi le son de l’ordinateur en réunion.' : 'Kobi n’écoute plus le son de l’ordinateur.');
      } catch (err) {
        ctx.notifier(message(err), 'erreur');
      }
    };
    el.replaceChildren(h('p', { class: 'intro' }, EXPLICATION), h('p', { class: 'chargement' }, 'Je regarde les sorties audio…'));
    invoke<EtatSonOrdi>('son_ordi').then(charger, (err) => {
      el.replaceChildren(h('p', { class: absente(err) ? 'aide' : 'erreur' }, absente(err) ? 'Ce réglage arrive avec la prochaine version de Kobi.' : message(err)));
      ctx.signaler?.('attente', absente(err) ? 'bientôt' : 'erreur');
    });
  },
};
