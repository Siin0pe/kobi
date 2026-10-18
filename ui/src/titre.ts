// Titre qui se pose comme le mot de Kobi : brouillé en symboles, puis posé
// lettre par lettre, au rythme de la mascotte (`mascotte/glyphs.ts`).

import { animationsReduites } from './dom';
import { NOISE, scramble, settleStep } from './mascotte/glyphs';

const PAS_MS = 60;
const enCours = new WeakMap<HTMLElement, number>();
/** Lettre brouillée en cours (titres vivants). */
const brouilles = new WeakMap<HTMLElement, number>();

/** Écrit `texte` dans `el` en le brouillant d'abord (sauf animations réduites). */
export function poserTitre(el: HTMLElement, texte: string): void {
  cancelAnimationFrame(enCours.get(el) ?? 0);
  window.clearTimeout(brouilles.get(el));
  brouilles.delete(el);
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

/**
 * Fait vivre un titre posé : une lettre au hasard se brouille trois pas,
 * puis revient (comme le mot de Kobi au repos). Sans effet pendant la pose.
 */
export function brouillerUneLettre(el: HTMLElement, hasard = Math.random): void {
  const texte = el.getAttribute('aria-label');
  if (!texte || enCours.has(el) || brouilles.has(el) || animationsReduites()) return;
  const lettres = Array.from(texte);
  const places = lettres.flatMap((c, i) => (/\S/.test(c) ? [i] : []));
  if (!places.length) return;
  const i = places[Math.floor(hasard() * places.length)] ?? 0;
  let pas = 0;
  const peindre = () => {
    if (pas < 3) {
      const l = [...lettres];
      l[i] = NOISE[Math.floor(hasard() * NOISE.length)] ?? '#';
      el.textContent = l.join('');
      pas++;
      brouilles.set(el, window.setTimeout(peindre, PAS_MS + 10));
    } else {
      el.textContent = texte;
      brouilles.delete(el);
    }
  };
  peindre();
}

/**
 * Titres vivants : toutes les 7 à 13 s, une lettre d'un titre visible de
 * `racine` (sélecteur `selecteur`) se brouille. En pause quand la fenêtre
 * est masquée ou les animations réduites.
 */
export function vivreTitres(racine: HTMLElement, selecteur = '.titre'): void {
  let minuterie = 0;
  const suivant = () => {
    window.clearTimeout(minuterie);
    if (document.hidden) return;
    minuterie = window.setTimeout(() => {
      const visibles = [...racine.querySelectorAll<HTMLElement>(selecteur)].filter((t) => t.offsetParent !== null);
      const t = visibles[Math.floor(Math.random() * visibles.length)];
      if (t) brouillerUneLettre(t);
      suivant();
    }, 7_000 + Math.random() * 6_000);
  };
  document.addEventListener('visibilitychange', suivant);
  suivant();
}
