// Panneau « Démarrage » (KOB-143) : Kobi se lance à l'ouverture de session,
// en veille et sans fenêtre de bienvenue. L'état vit là où le système le
// lit (clé Run, autostart XDG, LaunchAgent), pas dans `config.toml`.
//
// Au premier lancement, la case est cochée par défaut (décision du
// propriétaire) : `demarrage_par_defaut` l'active si l'utilisateur n'a
// encore jamais choisi, ni dans l'installeur ni ici. Dans les Réglages, on
// montre seulement l'état.

import { h } from '../dom';
import { t } from '../i18n';
import { invoke, message } from '../ipc';
import type { Panneau } from './panneau';

export const DEMARRAGE: Panneau = {
  id: 'demarrage',
  get titre() {
    return t('reglages.demarrage.titre');
  },
  get sousTitre() {
    return t('reglages.demarrage.sousTitre');
  },
  monter(el, ctx) {
    const zone = h('div', { class: 'reglage' });
    el.replaceChildren(zone);
    const dessiner = (actif: boolean) => {
      const interrupteur = h(
        'button',
        {
          type: 'button',
          role: 'switch',
          class: 'interrupteur',
          'aria-checked': String(actif),
          'aria-label': t('reglages.demarrage.lancer'),
          onclick: async () => {
            try {
              const relu = await invoke<boolean>('regler_demarrage_systeme', { actif: !actif });
              dessiner(relu);
              ctx.notifier(relu ? t('reglages.demarrage.active') : t('reglages.demarrage.desactive'));
            } catch (e) {
              ctx.notifier(message(e), 'erreur');
            }
          },
        },
        h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
      );
      zone.replaceChildren(
        h('div', { class: 'reglage-titre' }, t('reglages.demarrage.lancer')),
        h('p', { class: 'aide' }, t('reglages.demarrage.aide')),
        h('div', { class: 'ligne-interrupteur' }, interrupteur, h('span', {}, actif ? t('reglages.demarrage.etatActive') : t('reglages.demarrage.etatDesactive'))),
      );
      ctx.signaler?.('ok', actif ? t('reglages.demarrage.resumeActive') : t('reglages.demarrage.resumeDesactive'));
    };
    invoke<boolean>(ctx.parcours ? 'demarrage_par_defaut' : 'demarrage_systeme').then(dessiner, (e) => {
      zone.replaceChildren(h('p', { class: 'erreur' }, message(e)));
      ctx.signaler?.('erreur', t('reglages.demarrage.resumeErreur'));
    });
  },
};
