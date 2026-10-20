// « À valider » (KOB-62), dans la section Activité de l'app Kobi : les cartes
// de validation en attente, avec leur détail complet (diff, commande,
// paramètres, réglage avant → après) et les mêmes boutons que la carte à côté
// de Kobi. Liste tenue par la coque (`cartes_validation`) et mise à jour par
// l'événement `carte` ; une carte tranchée (clic, voix, échéance) s'en va.

import { animationsReduites, h } from '../dom';
import { surLangue, t } from '../i18n';
import { ecouter, invoke, message } from '../ipc';
import { cible, genre, isValidation, renderDetail, type CarteValidation } from '../validation-detail';
import '../styles/validation-detail.css';
import './validations.css';

/** Bloc « À valider » à poser dans la section Activité ; masqué sans carte. */
export function blocValidations(): HTMLElement {
  const liste = h('div', { class: 'validations-liste' });
  const titre = h('h2', { class: 'groupe' });
  const aide = h('p', { class: 'aide' });
  const bloc = h('section', { class: 'validations', hidden: true }, titre, aide, liste);
  const textes = () => {
    bloc.setAttribute('aria-label', t('app.validations.aria'));
    titre.textContent = t('app.validations.titre');
    aide.textContent = t('app.validations.aide');
  };
  textes();
  // Langue changée (KOB-208) : le bloc et ses cartes se redessinent.
  surLangue(() => {
    textes();
    afficher();
  });
  const cartes = new Map<string, CarteValidation>();

  const decider = async (c: CarteValidation, accepte: boolean, ligne: HTMLElement): Promise<void> => {
    ligne.querySelectorAll('button').forEach((b) => (b.disabled = true));
    try {
      await invoke('decider_carte', { id: c.id, accepte });
    } catch (e) {
      ligne.append(h('p', { class: 'erreur' }, message(e)));
      return;
    }
    retirer(c.id);
  };

  const carte = (c: CarteValidation, i: number): HTMLElement => {
    const g = genre(c);
    const ligne = h(
      'article',
      { class: 'validation', 'data-id': c.id, 'data-genre': c.genre, style: `--i:${Math.min(i, 20)}` },
      h('p', { class: 'validation-genre' }, h('span', { class: 'validation-glyphe', 'aria-hidden': 'true' }, g.glyph), g.kicker),
      h('p', { class: 'validation-resume' }, c.resume),
      cible(c.detail) ? h('p', { class: 'validation-cible' }, cible(c.detail)) : null,
    );
    if (c.detail) ligne.append(renderDetail(c.detail));
    ligne.append(
      h(
        'div',
        { class: 'validation-boutons' },
        h('button', { type: 'button', class: 'bouton plein', onclick: () => void decider(c, true, ligne) }, t(c.genre === 'suppression' ? 'app.validations.supprimer' : 'app.validations.valider')),
        h('button', { type: 'button', class: 'bouton', onclick: () => void decider(c, false, ligne) }, t('app.validations.refuser')),
      ),
    );
    return ligne;
  };

  const afficher = (): void => {
    bloc.hidden = cartes.size === 0;
    liste.replaceChildren(...[...cartes.values()].map(carte));
  };

  const retirer = (id: string): void => {
    if (!cartes.delete(id)) return;
    const ligne = liste.querySelector<HTMLElement>(`[data-id="${CSS.escape(id)}"]`);
    if (!ligne || animationsReduites()) {
      afficher();
      return;
    }
    ligne.classList.add('partie');
    window.setTimeout(afficher, 320);
  };

  invoke<CarteValidation[]>('cartes_validation').then(
    (liste) => {
      for (const c of liste) cartes.set(c.id, c);
      afficher();
    },
    () => afficher(),
  );
  ecouter<unknown>('carte', (c) => {
    if (!isValidation(c)) return;
    if (c.statut) retirer(c.id);
    else if (!cartes.has(c.id)) {
      cartes.set(c.id, c);
      afficher();
    }
  });
  return bloc;
}
