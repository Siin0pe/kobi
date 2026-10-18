// Titre qui se pose comme le mot de Kobi : brouillé en symboles, puis posé
// lettre par lettre, au rythme de la mascotte (`mascotte/glyphs.ts`).

import { animationsReduites } from './dom';
import { scramble, settleStep } from './mascotte/glyphs';

const PAS_MS = 60;
const enCours = new WeakMap<HTMLElement, number>();

/** Écrit `texte` dans `el` en le brouillant d'abord (sauf animations réduites). */
export function poserTitre(el: HTMLElement, texte: string): void {
  cancelAnimationFrame(enCours.get(el) ?? 0);
  el.setAttribute('aria-label', texte);
  const lettres = Array.from(texte);
  if (animationsReduites() || lettres.length === 0) {
    el.textContent = texte;
    return;
  }
  // Au-delà de 40 lettres, on brouille par blocs pour rester sous ~1 s.
  const vitesse = Math.max(1, lettres.length / 40);
  const fin = settleStep(lettres.length);
  const debut = performance.now();
  const peindre = (k: number) => {
    el.textContent = scramble(lettres, k).join('');
  };
  const boucle = (now: number) => {
    const k = Math.floor(((now - debut) / PAS_MS) * vitesse);
    peindre(k);
    if (k < fin) enCours.set(el, requestAnimationFrame(boucle));
    else {
      el.textContent = texte;
      enCours.delete(el);
    }
  };
  peindre(0);
  enCours.set(el, requestAnimationFrame(boucle));
}
