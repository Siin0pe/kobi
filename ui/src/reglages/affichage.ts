// Panneau « Affichage » (KOB-169) : widget simple ou bulle détaillée, vue
// agrandie de la conversation (ouverture automatique, taille, place, texte,
// délai). Chaque choix s'applique tout de suite dans la fenêtre de Kobi,
// sans redémarrer (affichage.ts).

import { affichage, changerAffichage, type Affichage } from '../affichage';
import { h } from '../dom';
import { t } from '../i18n';
import { invoke, message } from '../ipc';
import type { Panneau } from './panneau';

type Verbosite = 'bavard' | 'normal' | 'discret';

/** Combien Kobi parle (KOB-215) : réglage de la coque (`voice.verbosite`), appliqué à chaud. */
const verbosites = (): ReadonlyArray<{ valeur: Verbosite; titre: string; detail: string }> => [
  { valeur: 'normal', titre: t('reglages.affichage.verbositeNormal'), detail: t('reglages.affichage.verbositeNormalDetail') },
  { valeur: 'discret', titre: t('reglages.affichage.verbositeDiscret'), detail: t('reglages.affichage.verbositeDiscretDetail') },
  { valeur: 'bavard', titre: t('reglages.affichage.verbositeBavard'), detail: t('reglages.affichage.verbositeBavardDetail') },
];

function blocVerbosite(notifier: (texte: string, ton?: 'ok' | 'erreur') => void): HTMLElement {
  const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': t('reglages.affichage.verbositeTitre') });
  const marquer = (v: Verbosite): void => {
    for (const b of choix.children) b.setAttribute('aria-checked', String(b.getAttribute('data-valeur') === v));
  };
  for (const o of verbosites()) {
    choix.append(
      h(
        'button',
        {
          type: 'button',
          role: 'radio',
          class: 'option',
          'data-valeur': o.valeur,
          'aria-checked': 'false',
          onclick: async () => {
            try {
              marquer(await invoke<Verbosite>('regler_verbosite_ui', { verbosite: o.valeur }));
              notifier(t('reglages.affichage.verbositeNotif', { choix: o.titre.toLowerCase() }));
            } catch (e) {
              notifier(message(e), 'erreur');
            }
          },
        },
        h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
        h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, o.titre), h('span', { class: 'option-detail' }, o.detail)),
      ),
    );
  }
  invoke<Verbosite>('verbosite_ui').then(marquer, () => marquer('normal'));
  return h(
    'div',
    { class: 'reglage', 'data-reglage': 'verbosite' },
    h('div', { class: 'reglage-titre' }, t('reglages.affichage.verbositeTitre')),
    h('p', { class: 'aide' }, t('reglages.affichage.verbositeAide')),
    choix,
  );
}

interface Groupe<K extends keyof Affichage> {
  cle: K;
  titre: string;
  aide: string;
  options: ReadonlyArray<{ valeur: Affichage[K]; titre: string; detail: string }>;
}

// Textes lus au rendu : la langue change à chaud (KOB-208).
const groupes = (): [
  Groupe<'mode'>,
  Groupe<'auto'>,
  Groupe<'taille'>,
  Groupe<'position'>,
  Groupe<'texte'>,
  Groupe<'delai'>,
  Groupe<'etat'>,
] => [
  {
    cle: 'mode',
    titre: t('reglages.affichage.modeTitre'),
    aide: t('reglages.affichage.modeAide'),
    options: [
      { valeur: 'simple', titre: t('reglages.affichage.modeSimple'), detail: t('reglages.affichage.modeSimpleDetail') },
      { valeur: 'detaille', titre: t('reglages.affichage.modeDetaille'), detail: t('reglages.affichage.modeDetailleDetail') },
    ],
  },
  {
    cle: 'auto',
    titre: t('reglages.affichage.autoTitre'),
    aide: t('reglages.affichage.autoAide'),
    options: [
      { valeur: 'jamais', titre: t('reglages.affichage.autoJamais'), detail: t('reglages.affichage.autoJamaisDetail') },
      { valeur: 'longues', titre: t('reglages.affichage.autoLongues'), detail: t('reglages.affichage.autoLonguesDetail') },
      { valeur: 'toujours', titre: t('reglages.affichage.autoToujours'), detail: t('reglages.affichage.autoToujoursDetail') },
    ],
  },
  {
    cle: 'taille',
    titre: t('reglages.affichage.tailleTitre'),
    aide: t('reglages.affichage.tailleAide'),
    options: [
      { valeur: 'petite', titre: t('reglages.affichage.taillePetite'), detail: '480 × 420' },
      { valeur: 'moyenne', titre: t('reglages.affichage.tailleMoyenne'), detail: '640 × 560' },
      { valeur: 'grande', titre: t('reglages.affichage.tailleGrande'), detail: '860 × 720' },
    ],
  },
  {
    cle: 'position',
    titre: t('reglages.affichage.positionTitre'),
    aide: t('reglages.affichage.positionAide'),
    options: [
      { valeur: 'a-cote', titre: t('reglages.affichage.positionACote'), detail: t('reglages.affichage.positionACoteDetail') },
      { valeur: 'centre', titre: t('reglages.affichage.positionCentre'), detail: t('reglages.affichage.positionCentreDetail') },
      { valeur: 'moitie-droite', titre: t('reglages.affichage.positionMoitieDroite'), detail: t('reglages.affichage.positionMoitieDroiteDetail') },
    ],
  },
  {
    cle: 'texte',
    titre: t('reglages.affichage.texteTitre'),
    aide: t('reglages.affichage.texteAide'),
    options: [
      { valeur: 'petit', titre: t('reglages.affichage.textePetit'), detail: '15 px' },
      { valeur: 'moyen', titre: t('reglages.affichage.texteMoyen'), detail: '18 px' },
      { valeur: 'grand', titre: t('reglages.affichage.texteGrand'), detail: '22 px' },
    ],
  },
  {
    cle: 'delai',
    titre: t('reglages.affichage.delaiTitre'),
    aide: t('reglages.affichage.delaiAide'),
    options: [
      { valeur: 10, titre: t('reglages.affichage.delai10'), detail: '' },
      { valeur: 30, titre: t('reglages.affichage.delai30'), detail: '' },
      { valeur: 60, titre: t('reglages.affichage.delai60'), detail: '' },
      { valeur: 0, titre: t('reglages.affichage.delaiJamais'), detail: '' },
    ],
  },
  {
    cle: 'etat',
    titre: t('reglages.affichage.etatTitre'),
    aide: t('reglages.affichage.etatAide'),
    options: [
      { valeur: 'texte', titre: t('reglages.affichage.etatOui'), detail: t('reglages.affichage.etatOuiDetail') },
      { valeur: 'aucun', titre: t('reglages.affichage.etatNon'), detail: t('reglages.affichage.etatNonDetail') },
    ],
  },
];

export const AFFICHAGE: Panneau = {
  id: 'affichage',
  get titre() {
    return t('reglages.affichage.titre');
  },
  get sousTitre() {
    return t('reglages.affichage.sousTitre');
  },
  monter(el, ctx) {
    const blocs = groupes().map((g) => {
      const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': g.titre });
      const marquer = (valeur: unknown): void => {
        for (const b of choix.children) b.setAttribute('aria-checked', String(b.getAttribute('data-valeur') === String(valeur)));
      };
      for (const o of g.options as ReadonlyArray<{ valeur: string | number; titre: string; detail: string }>) {
        choix.append(
          h(
            'button',
            {
              type: 'button',
              role: 'radio',
              class: 'option',
              'data-valeur': String(o.valeur),
              onclick: () => {
                changerAffichage({ [g.cle]: o.valeur } as Partial<Affichage>);
                marquer(o.valeur);
                ctx.notifier(t('reglages.affichage.notif', { reglage: g.titre, choix: o.titre.toLowerCase() }));
              },
            },
            h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
            h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, o.titre), o.detail ? h('span', { class: 'option-detail' }, o.detail) : ''),
          ),
        );
      }
      marquer(affichage()[g.cle]);
      return h('div', { class: 'reglage', 'data-reglage': g.cle }, h('div', { class: 'reglage-titre' }, g.titre), h('p', { class: 'aide' }, g.aide), choix);
    });
    el.replaceChildren(blocVerbosite(ctx.notifier), ...blocs);
    ctx.signaler?.('ok', affichage().mode === 'simple' ? t('reglages.affichage.resumeSimple') : t('reglages.affichage.resumeDetaille'));
  },
};
