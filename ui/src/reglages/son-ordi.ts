// Panneau « Son de l'ordinateur » (KOB-164) : en réunion, Kobi peut écouter
// aussi la sortie audio de l'ordinateur (la voix des autres en visio), sur
// une piste « eux » à côté du micro. Désactivé par défaut ; le changement
// vaut tout de suite, même en pleine réunion. Monté dans la section Réglages
// de l'app et proposé, décoché, au premier lancement.

import { h } from '../dom';
import { t } from '../i18n';
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


function dessiner(el: HTMLElement, e: EtatSonOrdi, ctx: Contexte, regler: (actif: boolean, sortie: string | null) => void): void {
  const interrupteur = h(
    'button',
    {
      type: 'button',
      role: 'switch',
      class: 'interrupteur',
      'aria-checked': String(e.actif),
      'aria-label': t('reglages.sonOrdi.ecouter'),
      onclick: () => regler(!e.actif, e.sortie),
    },
    h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
  );
  const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': t('reglages.sonOrdi.sortieEcoutee') });
  const options: Array<{ nom: string | null; libelle: string; detail?: string }> = [
    { nom: null, libelle: t('reglages.sonOrdi.systeme'), detail: e.defaut ?? t('reglages.sonOrdi.sortieParDefaut') },
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
    h('p', { class: 'intro' }, t('reglages.sonOrdi.explication')),
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'son-ordi' },
      h('div', { class: 'reglage-titre' }, t('reglages.sonOrdi.ecouter')),
      h('p', { class: 'aide' }, t('reglages.sonOrdi.aide')),
      h(
        'div',
        { class: 'ligne-interrupteur' },
        interrupteur,
        h('span', {}, e.actif ? (e.capte ? t('reglages.sonOrdi.etatCapte') : t('reglages.sonOrdi.etatActive')) : t('reglages.sonOrdi.etatDesactive')),
      ),
    ),
  );
  if (e.actif) {
    el.append(
      h(
        'div',
        { class: 'reglage', 'data-reglage': 'sortie' },
        h('div', { class: 'reglage-titre' }, t('reglages.sonOrdi.sortieTitre')),
        h('p', { class: 'aide' }, t('reglages.sonOrdi.sortieAide')),
        choix,
      ),
    );
  }
  ctx.signaler?.(e.actif ? 'ok' : 'attente', e.actif ? t('reglages.sonOrdi.resumeActive') : t('reglages.sonOrdi.resumeDesactive'));
}

export const SON_ORDI: Panneau = {
  id: 'son-ordi',
  get titre() {
    return t('reglages.sonOrdi.titre');
  },
  get sousTitre() {
    return t('reglages.sonOrdi.sousTitre');
  },
  monter(el, ctx) {
    const charger = (e: EtatSonOrdi) => dessiner(el, e, ctx, regler);
    const regler = async (actif: boolean, sortie: string | null) => {
      try {
        const e = await invoke<EtatSonOrdi>('regler_son_ordi', { actif, sortie });
        charger(e);
        ctx.notifier(e.actif ? t('reglages.sonOrdi.active') : t('reglages.sonOrdi.desactive'));
      } catch (err) {
        ctx.notifier(message(err), 'erreur');
      }
    };
    el.replaceChildren(h('p', { class: 'intro' }, t('reglages.sonOrdi.explication')), h('p', { class: 'chargement' }, t('reglages.sonOrdi.chargement')));
    invoke<EtatSonOrdi>('son_ordi').then(charger, (err) => {
      el.replaceChildren(h('p', { class: absente(err) ? 'aide' : 'erreur' }, absente(err) ? t('reglages.sonOrdi.bientot') : message(err)));
      ctx.signaler?.('attente', absente(err) ? t('reglages.sonOrdi.resumeBientot') : t('reglages.sonOrdi.resumeErreur'));
    });
  },
};
