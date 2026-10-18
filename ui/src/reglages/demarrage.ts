// Panneau « Démarrage » (KOB-143) : Kobi se lance à l'ouverture de session,
// en veille et sans fenêtre de bienvenue. L'état vit là où le système le
// lit (clé Run, autostart XDG, LaunchAgent), pas dans `config.toml`.
//
// Au premier lancement, la case est cochée par défaut (décision du
// propriétaire) : `demarrage_par_defaut` l'active si l'utilisateur n'a
// encore jamais choisi, ni dans l'installeur ni ici. Dans les Réglages, on
// montre seulement l'état.

import { h } from '../dom';
import { invoke, message } from '../ipc';
import type { Panneau } from './panneau';

export const DEMARRAGE: Panneau = {
  id: 'demarrage',
  titre: 'Démarrage',
  sousTitre: 'Kobi à l’ouverture de session',
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
          'aria-label': 'Lancer Kobi à l’ouverture de session',
          onclick: async () => {
            try {
              const relu = await invoke<boolean>('regler_demarrage_systeme', { actif: !actif });
              dessiner(relu);
              ctx.notifier(relu ? 'Kobi démarrera avec ta session.' : 'Kobi ne démarrera plus tout seul.');
            } catch (e) {
              ctx.notifier(message(e), 'erreur');
            }
          },
        },
        h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
      );
      zone.replaceChildren(
        h('div', { class: 'reglage-titre' }, 'Lancer Kobi à l’ouverture de session'),
        h('p', { class: 'aide' }, 'Il démarre en veille, sans fenêtre : il suffit ensuite de dire « Kobi ».'),
        h('div', { class: 'ligne-interrupteur' }, interrupteur, h('span', {}, actif ? 'Activé' : 'Désactivé')),
      );
      ctx.signaler?.('ok', actif ? 'activé' : 'désactivé');
    };
    invoke<boolean>(ctx.parcours ? 'demarrage_par_defaut' : 'demarrage_systeme').then(dessiner, (e) => {
      zone.replaceChildren(h('p', { class: 'erreur' }, message(e)));
      ctx.signaler?.('erreur', 'erreur');
    });
  },
};
