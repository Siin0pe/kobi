// Panneau « Permissions » (KOB-215) : ce que Kobi fait sans demander. La case
// « Ne plus demander » d'une carte de validation se règle et se révoque ici ;
// l'état vit dans `[permissions]` de `config.toml`. Une suppression se
// demande toujours, et chaque service (MCP) a son propre choix dans
// Réglages › Outils MCP.

import { h } from '../dom';
import { t } from '../i18n';
import { invoke, message } from '../ipc';
import type { Panneau } from './panneau';

interface Permissions {
  fichier: boolean;
  commande: boolean;
  reglage: boolean;
}

/** Libellés évalués au rendu : la langue change à chaud. */
function genres(): Array<{ genre: keyof Permissions; titre: string; aide: string }> {
  return [
    { genre: 'fichier', titre: t('reglages.permissions.fichier.titre'), aide: t('reglages.permissions.fichier.aide') },
    { genre: 'commande', titre: t('reglages.permissions.commande.titre'), aide: t('reglages.permissions.commande.aide') },
    { genre: 'reglage', titre: t('reglages.permissions.reglage.titre'), aide: t('reglages.permissions.reglage.aide') },
  ];
}

export const PERMISSIONS: Panneau = {
  id: 'permissions',
  get titre() {
    return t('reglages.permissions.titre');
  },
  get sousTitre() {
    return t('reglages.permissions.sousTitre');
  },
  monter(el, ctx) {
    const zone = h('div', { class: 'reglage' });
    el.replaceChildren(zone);
    const dessiner = (p: Permissions) => {
      zone.replaceChildren(
        h('p', { class: 'aide' }, t('reglages.permissions.intro')),
        ...genres().map(({ genre, titre, aide }) =>
          h(
            'div',
            { class: 'ligne-interrupteur', 'data-genre': genre },
            h(
              'button',
              {
                type: 'button',
                role: 'switch',
                class: 'interrupteur',
                'aria-checked': String(p[genre]),
                'aria-label': t('reglages.permissions.libelle', { titre }),
                onclick: async () => {
                  try {
                    const relu = await invoke<Permissions>('regler_permission', { genre, sansDemander: !p[genre] });
                    dessiner(relu);
                    ctx.notifier(relu[genre] ? t('reglages.permissions.sansDemanderFait', { titre }) : t('reglages.permissions.demanderFait', { titre }));
                  } catch (e) {
                    ctx.notifier(message(e), 'erreur');
                  }
                },
              },
              h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
            ),
            h('span', {}, h('strong', {}, titre), ` · ${p[genre] ? t('reglages.permissions.sansDemander') : t('reglages.permissions.demander')}`, h('br'), h('small', { class: 'aide' }, aide)),
          ),
        ),
      );
    };
    invoke<Permissions>('permissions').then(dessiner, (e) => zone.replaceChildren(h('p', { class: 'erreur' }, message(e))));
  },
};
