// Panneau « Animations » (KOB-149) : Kobi toujours animé (par défaut), qui
// suit le réglage de Windows, ou aux animations réduites. Composant autonome,
// branché dans la fenêtre Réglages en attendant la section Réglages de l'app
// Kobi (KOB-159). Avec les images par seconde et l'économie d'énergie
// (KOB-200), gardées dans config.toml (`animations.fps`, `animations.economie`).

import { choisirAnimations, modeAnimations, systemeReduit, type ModeAnimations } from '../animations';
import { CHOIX_FPS, poserReglageAnimations, reglageAnimations, suivreReglageAnimations, type Fps } from '../cadence';
import { h } from '../dom';
import { disponible, invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

const OPTIONS: ReadonlyArray<{ mode: ModeAnimations; titre: string; detail: () => string }> = [
  { mode: 'toujours', titre: 'Toujours', detail: () => 'Kobi bouge en permanence, quel que soit le réglage de Windows.' },
  {
    mode: 'systeme',
    titre: 'Comme Windows',
    detail: () => (systemeReduit() ? 'Windows a coupé ses effets d’animation : Kobi resterait immobile.' : 'Windows anime ses fenêtres : Kobi aussi.'),
  },
  { mode: 'reduites', titre: 'Réduites', detail: () => 'Kobi garde ses expressions, sans mouvement.' },
];

const DETAIL_FPS: Record<string, string> = {
  '5': 'Le plus sobre : le mouvement avance par petits pas.',
  '15': 'Sobre, encore lisible.',
  '30': 'Fluide, moitié moins d’images.',
  '60': 'Fluide (par défaut).',
  '120': 'Pour les écrans à 120 Hz et plus.',
  ecran: 'Autant d’images que l’écran en affiche.',
};

/** Change un réglage d'animation dans config.toml (hors de Kobi : cette fenêtre seulement). */
function regler(cle: 'fps' | 'economie', valeur: Fps | boolean, ctx: Contexte): void {
  if (!disponible()) {
    poserReglageAnimations({ [cle]: valeur });
    return;
  }
  invoke<{ fps: Fps; economie: boolean }>('regler_animations', { cle, valeur }).then(
    (r) => poserReglageAnimations(r),
    (e) => ctx.notifier(`Réglage non changé : ${message(e)}`, 'erreur'),
  );
}

/** « Images par seconde » et « Économie d'énergie » (KOB-200). */
function blocCadence(ctx: Contexte): HTMLElement {
  const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': 'Images par seconde' });
  for (const fps of CHOIX_FPS) {
    const titre = fps === 'ecran' ? 'Suivre l’écran' : `${fps} images/s`;
    choix.append(
      h(
        'button',
        {
          type: 'button',
          role: 'radio',
          class: 'option',
          'data-fps': String(fps),
          onclick: () => {
            regler('fps', fps, ctx);
            ctx.notifier(`Images par seconde : ${titre.toLowerCase()}`);
          },
        },
        h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
        h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, titre), h('span', { class: 'option-detail' }, DETAIL_FPS[String(fps)]!)),
      ),
    );
  }
  const interrupteur = h(
    'button',
    {
      type: 'button',
      role: 'switch',
      class: 'interrupteur',
      'aria-label': 'Économie d’énergie',
      onclick: () => regler('economie', !reglageAnimations().economie, ctx),
    },
    h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
  );
  const etat = h('span', {});
  const afficher = (): void => {
    const r = reglageAnimations();
    for (const b of choix.children) b.setAttribute('aria-checked', String(b.getAttribute('data-fps') === String(r.fps)));
    interrupteur.setAttribute('aria-checked', String(r.economie));
    etat.textContent = r.economie ? 'Activée' : 'Désactivée';
  };
  afficher();
  // Le panneau est monté une fois pour la vie de la fenêtre : il suit le réglage, d'où qu'il change.
  suivreReglageAnimations(afficher);
  return h(
    'div',
    {},
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'fps' },
      h('div', { class: 'reglage-titre' }, 'Images par seconde'),
      h('p', { class: 'aide' }, 'Tout ce qui bouge suit ce nombre : la mascotte, ses bulles, la conversation en grand et l’app.'),
      choix,
    ),
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'economie' },
      h('div', { class: 'reglage-titre' }, 'Économie d’énergie'),
      h('p', { class: 'aide' }, 'Au repos prolongé, Kobi ralentit à 3 puis 2 images par seconde pour ménager la batterie. Le mouvement devient saccadé.'),
      h('div', { class: 'ligne-interrupteur' }, interrupteur, etat),
    ),
  );
}

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
      blocCadence(ctx),
    );
    selection(modeAnimations());
  },
};
