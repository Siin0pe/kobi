// Panneau « Clé API » (KOB-104) : la clé Anthropic du backend sans Claude
// Code (KOB-101). Elle va dans le gestionnaire d'identifiants du système,
// jamais dans `config.toml` ; la coque ne renvoie que ses quatre derniers
// caractères, et le champ est vidé dès qu'elle est enregistrée.

import { h } from '../dom';
import { t } from '../i18n';
import { invoke, message } from '../ipc';
import type { Panneau } from './panneau';

/** État renvoyé par la coque : jamais la clé elle-même. */
interface EtatCle {
  presente: boolean;
  fin: string | null;
}

export const CLE_API: Panneau = {
  id: 'cle-api',
  get titre() {
    return t('reglages.cleApi.titre');
  },
  get sousTitre() {
    return t('reglages.cleApi.sousTitre');
  },
  monter(el, ctx) {
    const zone = h('div', { class: 'reglage', 'data-reglage': 'cle-api' });
    el.replaceChildren(zone);
    const dessiner = (e: EtatCle) => {
      const champ = h('input', {
        type: 'password',
        name: 'cle-api',
        class: 'cle-api-champ',
        autocomplete: 'off',
        spellcheck: 'false',
        placeholder: 'sk-ant-…',
        'aria-label': t('reglages.cleApi.champ'),
      }) as HTMLInputElement;
      const enregistrer = async () => {
        const cle = champ.value;
        champ.value = '';
        try {
          dessiner(await invoke<EtatCle>('cle_api_enregistrer', { cle }));
          ctx.notifier(t('reglages.cleApi.enregistree'));
        } catch (err) {
          ctx.notifier(message(err), 'erreur');
        }
      };
      champ.addEventListener('keydown', (ev) => {
        if (ev.key === 'Enter') void enregistrer();
      });
      zone.replaceChildren(
        h('div', { class: 'reglage-titre' }, t('reglages.cleApi.titre')),
        h('p', { class: 'aide' }, t('reglages.cleApi.aide')),
        h(
          'p',
          { class: 'cle-api-etat', 'data-presente': String(e.presente) },
          e.presente ? t('reglages.cleApi.presente', { fin: e.fin ?? '' }) : t('reglages.cleApi.absente'),
        ),
        h(
          'div',
          { class: 'boutons' },
          champ,
          h(
            'button',
            { type: 'button', class: 'bouton plein', 'data-action': 'enregistrer', onclick: () => void enregistrer() },
            e.presente ? t('reglages.cleApi.remplacer') : t('reglages.cleApi.enregistrer'),
          ),
          e.presente &&
            h(
              'button',
              {
                type: 'button',
                class: 'bouton discret',
                'data-action': 'supprimer',
                onclick: async () => {
                  try {
                    dessiner(await invoke<EtatCle>('cle_api_supprimer'));
                    ctx.notifier(t('reglages.cleApi.supprimee'));
                  } catch (err) {
                    ctx.notifier(message(err), 'erreur');
                  }
                },
              },
              t('reglages.cleApi.supprimer'),
            ),
        ),
      );
      ctx.signaler?.(e.presente ? 'ok' : 'attente', e.presente ? t('reglages.cleApi.resumePresente') : t('reglages.cleApi.resumeAbsente'));
    };
    invoke<EtatCle>('cle_api_etat').then(dessiner, (err) => {
      zone.replaceChildren(h('p', { class: 'erreur' }, message(err)));
      ctx.signaler?.('erreur', t('reglages.cleApi.resumeErreur'));
    });
  },
};
