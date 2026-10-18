// Réglage « Animations de Kobi » (KOB-149) : toujours, suivre Windows, ou
// réduites. Par défaut, toujours : sur un PC en « meilleures performances »,
// Windows coupe ses effets d'animation et WebView2 annonce alors
// `prefers-reduced-motion: reduce` ; Kobi était entièrement figé sans que
// l'utilisateur l'ait demandé (décision du propriétaire, 18/10/2026).
//
// Préférence d'interface gardée dans le stockage local de l'app : la mascotte
// et les autres fenêtres partagent la même origine, et l'événement `storage`
// prévient chaque fenêtre ouverte dès que le réglage change.

export type ModeAnimations = 'toujours' | 'systeme' | 'reduites';

export const MODES: readonly ModeAnimations[] = ['toujours', 'systeme', 'reduites'];
export const CLE_ANIMATIONS = 'kobi.animations';
export const MODE_PAR_DEFAUT: ModeAnimations = 'toujours';

const requete = (): MediaQueryList => window.matchMedia('(prefers-reduced-motion: reduce)');

function estMode(v: unknown): v is ModeAnimations {
  return typeof v === 'string' && (MODES as readonly string[]).includes(v);
}

export function modeAnimations(): ModeAnimations {
  try {
    const v = window.localStorage.getItem(CLE_ANIMATIONS);
    return estMode(v) ? v : MODE_PAR_DEFAUT;
  } catch {
    return MODE_PAR_DEFAUT;
  }
}

export function choisirAnimations(mode: ModeAnimations): void {
  try {
    window.localStorage.setItem(CLE_ANIMATIONS, mode);
  } catch {
    // Stockage indisponible : le choix vaut pour cette fenêtre seulement.
  }
  ecouteurs.forEach((fn) => fn());
}

/** Le réglage système demande de réduire les animations. */
export function systemeReduit(): boolean {
  return requete().matches;
}

/** Faut-il réduire les animations, d'après le réglage de Kobi (et Windows en mode « suivre ») ? */
export function animationsReduitesSelonReglage(): boolean {
  const mode = modeAnimations();
  return mode === 'reduites' || (mode === 'systeme' && systemeReduit());
}

const ecouteurs = new Set<() => void>();

/**
 * Fenêtres secondaires (app Kobi, Carnet, Réglages, premier lancement,
 * notes) : la classe `reduced-motion` sur `<html>` suit le réglage ; leurs
 * feuilles de style et `animationsReduites()` (dom.ts) la lisent. Au
 * chargement, elle n'est que posée si besoin (jamais retirée) : une page qui
 * la pose elle-même la garde. Ensuite, chaque changement du réglage la tient à jour.
 */
let classeSuivie = false;
export function suivreClasseAnimations(): void {
  if (classeSuivie || typeof document === 'undefined') return;
  classeSuivie = true;
  if (animationsReduitesSelonReglage()) document.documentElement.classList.add('reduced-motion');
  suivreAnimations((reduites) => document.documentElement.classList.toggle('reduced-motion', reduites));
}

/**
 * Appelle `fn(reduites)` à chaque changement du réglage (ici ou dans une autre
 * fenêtre) ou, en mode « suivre Windows », du réglage système.
 */
export function suivreAnimations(fn: (reduites: boolean) => void): () => void {
  let avant = animationsReduitesSelonReglage();
  const verifier = (): void => {
    const maintenant = animationsReduitesSelonReglage();
    if (maintenant !== avant) {
      avant = maintenant;
      fn(maintenant);
    }
  };
  const surStockage = (e: StorageEvent): void => {
    if (e.key === CLE_ANIMATIONS || e.key === null) verifier();
  };
  const mq = requete();
  mq.addEventListener('change', verifier);
  window.addEventListener('storage', surStockage);
  ecouteurs.add(verifier);
  return () => {
    mq.removeEventListener('change', verifier);
    window.removeEventListener('storage', surStockage);
    ecouteurs.delete(verifier);
  };
}
