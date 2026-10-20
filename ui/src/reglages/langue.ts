// Panneau « Langue » (KOB-207) : la langue de l'interface, celle du système
// par défaut. Le choix est gardé dans config.toml (`ui.language`) et toutes
// les fenêtres se traduisent à chaud ; ce panneau aussi.

import { h } from '../dom';
import { choisirLangue, langue, langueDuSysteme, LANGUES, reglee, surLangue, t, type Langue } from '../i18n';
import { message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

function nomDe(l: Langue): string {
  return LANGUES.find((x) => x.code === l)!.nom;
}

export const LANGUE: Panneau = {
  id: 'langue',
  get titre() {
    return t('langue.titre');
  },
  get sousTitre() {
    return t('langue.sousTitre');
  },
  monter(el: HTMLElement, ctx: Contexte) {
    // `null` : celle du système ; `undefined` : pas encore lue.
    let choix: Langue | null | undefined;
    const dessiner = (): void => {
      const options: Array<{ code: Langue | null; titre: string; detail: string }> = [
        { code: null, titre: t('langue.systeme'), detail: t('langue.systemeDetail', { nom: nomDe(langueDuSysteme()) }) },
        ...LANGUES.map((l) => ({ code: l.code, titre: l.nom, detail: '' })),
      ];
      const groupe = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': t('langue.titre') });
      for (const o of options) {
        groupe.append(
          h(
            'button',
            {
              type: 'button',
              role: 'radio',
              class: 'option',
              'data-langue': o.code ?? 'systeme',
              'aria-checked': String(choix !== undefined && choix === o.code),
              onclick: () => {
                choisirLangue(o.code).then(
                  (l) => {
                    choix = o.code;
                    dessiner();
                    ctx.notifier(t('langue.choisie', { nom: nomDe(l) }));
                  },
                  (e) => ctx.notifier(message(e), 'erreur'),
                );
              },
            },
            h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
            h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, o.titre), o.detail ? h('span', { class: 'option-detail' }, o.detail) : ''),
          ),
        );
      }
      el.replaceChildren(groupe, h('p', { class: 'aide' }, t('langue.aide')));
      ctx.signaler?.('ok', choix === null || choix === undefined ? t('langue.systeme') : nomDe(choix));
    };
    dessiner();
    reglee().then(
      (l) => {
        choix = l;
        dessiner();
      },
      () => {
        // Hors de Kobi : la langue de cette fenêtre.
        choix = langue();
        dessiner();
      },
    );
    // Changée ailleurs (autre fenêtre, `kobi config set ui.language`) : le
    // choix coché suit la config.
    return surLangue(() => {
      dessiner();
      reglee().then(
        (l) => {
          choix = l;
          dessiner();
        },
        () => {},
      );
    });
  },
};
