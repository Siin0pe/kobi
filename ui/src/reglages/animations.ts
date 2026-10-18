// Panneau « Animations » (KOB-149) : Kobi toujours animé (par défaut), qui
// suit le réglage de Windows, ou aux animations réduites. Composant autonome,
// branché dans la fenêtre Réglages en attendant la section Réglages de l'app
// Kobi (KOB-159).

import { choisirAnimations, modeAnimations, systemeReduit, type ModeAnimations } from '../animations';
import { h } from '../dom';
import type { Panneau } from './panneau';

const OPTIONS: ReadonlyArray<{ mode: ModeAnimations; titre: string; detail: () => string }> = [
  { mode: 'toujours', titre: 'Toujours', detail: () => 'Kobi bouge en permanence, quel que soit le réglage de Windows.' },
  {
    mode: 'systeme',
    titre: 'Comme Windows',
    detail: () => (systemeReduit() ? 'Windows a coupé ses effets d’animation : Kobi resterait immobile.' : 'Windows anime ses fenêtres : Kobi aussi.'),
  },
  { mode: 'reduites', titre: 'Réduites', detail: () => 'Kobi garde ses expressions, sans mouvement.' },
];

export const ANIMATIONS: Panneau = {
  id: 'animations',
  titre: 'Animations',
  sousTitre: 'Kobi qui bouge, ou immobile',
  monter(el, ctx) {
    const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': 'Animations de Kobi' });
    const selection = (mode: ModeAnimations) => {
      for (const b of choix.children) b.setAttribute('aria-checked', String(b.getAttribute('data-mode') === mode));
      ctx.signaler?.('ok', OPTIONS.find((o) => o.mode === mode)!.titre.toLowerCase());
    };
    for (const o of OPTIONS) {
      choix.append(
        h(
          'button',
          {
            type: 'button',
            role: 'radio',
            class: 'option',
            'data-mode': o.mode,
            onclick: () => {
              choisirAnimations(o.mode);
              selection(o.mode);
              ctx.notifier(`Animations : ${o.titre.toLowerCase()}`);
            },
          },
          h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
          h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, o.titre), h('span', { class: 'option-detail' }, o.detail())),
        ),
      );
    }
    el.replaceChildren(
      h(
        'div',
        { class: 'reglage', 'data-reglage': 'animations' },
        h('div', { class: 'reglage-titre' }, 'Animations de Kobi'),
        h('p', { class: 'aide' }, 'La mascotte, ses bulles et le texte. Le changement vaut tout de suite, dans toutes les fenêtres.'),
        choix,
      ),
    );
    selection(modeAnimations());
  },
};
