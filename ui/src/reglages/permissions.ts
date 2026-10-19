// Panneau « Permissions » (KOB-215) : ce que Kobi fait sans demander. La case
// « Ne plus demander » d'une carte de validation se règle et se révoque ici ;
// l'état vit dans `[permissions]` de `config.toml`. Une suppression se
// demande toujours, et chaque service (MCP) a son propre choix dans
// Réglages › Outils MCP.

import { h } from '../dom';
import { invoke, message } from '../ipc';
import type { Panneau } from './panneau';

interface Permissions {
  fichier: boolean;
  commande: boolean;
  reglage: boolean;
}

const GENRES: Array<{ genre: keyof Permissions; titre: string; aide: string }> = [
  { genre: 'fichier', titre: 'Écrire des fichiers', aide: 'Créer ou modifier un fichier dans tes dossiers de code.' },
  { genre: 'commande', titre: 'Lancer des commandes', aide: 'Tests, compilation, scripts… dans tes dossiers de code.' },
  { genre: 'reglage', titre: 'Changer ses réglages', aide: 'Quand tu le lui demandes, ou un agent que tu as branché.' },
];

export const PERMISSIONS: Panneau = {
  id: 'permissions',
  titre: 'Permissions',
  sousTitre: 'Ce que Kobi fait sans demander',
  monter(el, ctx) {
    const zone = h('div', { class: 'reglage' });
    el.replaceChildren(zone);
    const dessiner = (p: Permissions) => {
      zone.replaceChildren(
        h('p', { class: 'aide' }, 'Par défaut, Kobi te montre une carte avant tout ce qui écrit. Une suppression est toujours demandée.'),
        ...GENRES.map(({ genre, titre, aide }) =>
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
                'aria-label': `${titre} sans demander`,
                onclick: async () => {
                  try {
                    const relu = await invoke<Permissions>('regler_permission', { genre, sansDemander: !p[genre] });
                    dessiner(relu);
                    ctx.notifier(relu[genre] ? `${titre} : sans demander.` : `${titre} : Kobi demandera.`);
                  } catch (e) {
                    ctx.notifier(message(e), 'erreur');
                  }
                },
              },
              h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
            ),
            h('span', {}, h('strong', {}, titre), ` · ${p[genre] ? 'sans demander' : 'demander'}`, h('br'), h('small', { class: 'aide' }, aide)),
          ),
        ),
      );
    };
    invoke<Permissions>('permissions').then(dessiner, (e) => zone.replaceChildren(h('p', { class: 'erreur' }, message(e))));
  },
};
