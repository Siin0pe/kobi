// Panneau « Animations » (KOB-149) : Kobi toujours animé (par défaut), qui
// suit le réglage de Windows, ou aux animations réduites. Composant autonome,
// branché dans la fenêtre Réglages en attendant la section Réglages de l'app
// Kobi (KOB-159). Avec les images par seconde et l'économie d'énergie
// (KOB-200), gardées dans config.toml (`animations.fps`, `animations.economie`).

import { choisirAnimations, modeAnimations, systemeReduit, type ModeAnimations } from '../animations';
import { CHOIX_FPS, poserReglageAnimations, reglageAnimations, suivreReglageAnimations, type Fps } from '../cadence';
import { h } from '../dom';
import { t } from '../i18n';
import { disponible, invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

// Textes lus au rendu : la langue change à chaud (KOB-208).
const options = (): ReadonlyArray<{ mode: ModeAnimations; titre: string; detail: () => string }> => [
  { mode: 'toujours', titre: t('reglages.animations.toujours'), detail: () => t('reglages.animations.toujoursDetail') },
  {
    mode: 'systeme',
    titre: t('reglages.animations.systeme'),
    detail: () => (systemeReduit() ? t('reglages.animations.systemeDetailReduit') : t('reglages.animations.systemeDetail')),
  },
  { mode: 'reduites', titre: t('reglages.animations.reduites'), detail: () => t('reglages.animations.reduitesDetail') },
];

const DETAIL_FPS: Record<string, string> = {
  '5': 'reglages.animations.fps5',
  '15': 'reglages.animations.fps15',
  '30': 'reglages.animations.fps30',
  '60': 'reglages.animations.fps60',
  '120': 'reglages.animations.fps120',
  ecran: 'reglages.animations.fpsEcran',
};

/** Change un réglage d'animation dans config.toml (hors de Kobi : cette fenêtre seulement). */
function regler(cle: 'fps' | 'economie', valeur: Fps | boolean, ctx: Contexte): void {
  if (!disponible()) {
    poserReglageAnimations({ [cle]: valeur });
    return;
  }
  invoke<{ fps: Fps; economie: boolean }>('regler_animations', { cle, valeur }).then(
    (r) => poserReglageAnimations(r),
    (e) => ctx.notifier(t('reglages.animations.erreur', { erreur: message(e) }), 'erreur'),
  );
}

/** « Images par seconde » et « Économie d'énergie » (KOB-200). */
function blocCadence(ctx: Contexte): HTMLElement {
  const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': t('reglages.animations.fpsTitre') });
  for (const fps of CHOIX_FPS) {
    const titre = fps === 'ecran' ? t('reglages.animations.fpsSuivreEcran') : t('reglages.animations.fpsOption', { fps });
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
            ctx.notifier(t('reglages.animations.fpsNotif', { choix: titre.toLowerCase() }));
          },
        },
        h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
        h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, titre), h('span', { class: 'option-detail' }, t(DETAIL_FPS[String(fps)]!))),
      ),
    );
  }
  const interrupteur = h(
    'button',
    {
      type: 'button',
      role: 'switch',
      class: 'interrupteur',
      'aria-label': t('reglages.animations.economieTitre'),
      onclick: () => regler('economie', !reglageAnimations().economie, ctx),
    },
    h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
  );
  const etat = h('span', {});
  const afficher = (): void => {
    const r = reglageAnimations();
    for (const b of choix.children) b.setAttribute('aria-checked', String(b.getAttribute('data-fps') === String(r.fps)));
    interrupteur.setAttribute('aria-checked', String(r.economie));
    etat.textContent = r.economie ? t('reglages.animations.economieActivee') : t('reglages.animations.economieDesactivee');
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
      h('div', { class: 'reglage-titre' }, t('reglages.animations.fpsTitre')),
      h('p', { class: 'aide' }, t('reglages.animations.fpsAide')),
      choix,
    ),
    h(
      'div',
      { class: 'reglage', 'data-reglage': 'economie' },
      h('div', { class: 'reglage-titre' }, t('reglages.animations.economieTitre')),
      h('p', { class: 'aide' }, t('reglages.animations.economieAide')),
      h('div', { class: 'ligne-interrupteur' }, interrupteur, etat),
    ),
  );
}

export const ANIMATIONS: Panneau = {
  id: 'animations',
  get titre() {
    return t('reglages.animations.titre');
  },
  get sousTitre() {
    return t('reglages.animations.sousTitre');
  },
  monter(el, ctx) {
    const OPTIONS = options();
    const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': t('reglages.animations.modeTitre') });
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
              ctx.notifier(t('reglages.animations.modeNotif', { mode: o.titre.toLowerCase() }));
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
        h('div', { class: 'reglage-titre' }, t('reglages.animations.modeTitre')),
        h('p', { class: 'aide' }, t('reglages.animations.modeAide')),
        choix,
      ),
      blocCadence(ctx),
    );
    selection(modeAnimations());
  },
};
