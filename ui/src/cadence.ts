// Images par seconde (KOB-200) : réglage « Images par seconde » de l'app
// (5, 15, 30, 60, 120 ou suivre l'écran ; défaut 60) et « Économie
// d'énergie » (désactivée par défaut). Tout ce qui bouge suit ce nombre : la
// mascotte, les bulles, la vue agrandie et l'app (econome.ts).
//
// Les réglages vivent dans `config.toml` (`animations.fps`,
// `animations.economie`, `kobi config set`) : chaque fenêtre les lit au
// chargement, puis suit l'événement `reglage` que la coque envoie à chaque
// changement, d'où qu'il vienne.

import { disponible, ecouter, invoke } from './ipc';

export type Fps = 5 | 15 | 30 | 60 | 120 | 'ecran';
export const CHOIX_FPS: readonly Fps[] = [5, 15, 30, 60, 120, 'ecran'];
export const FPS_PAR_DEFAUT: Fps = 60;

export interface ReglageAnimations {
  fps: Fps;
  economie: boolean;
}

function estFps(v: unknown): v is Fps {
  return (CHOIX_FPS as readonly unknown[]).includes(v);
}

let reglage: ReglageAnimations = { fps: FPS_PAR_DEFAUT, economie: false };
const ecouteurs = new Set<(r: ReglageAnimations) => void>();

export function reglageAnimations(): ReglageAnimations {
  return reglage;
}

/** Nouveau réglage (coque, Réglages de l'app, tests) : prévient chaque abonné. */
export function poserReglageAnimations(r: Partial<ReglageAnimations>): void {
  const fps = estFps(r.fps) ? r.fps : reglage.fps;
  const economie = typeof r.economie === 'boolean' ? r.economie : reglage.economie;
  if (fps === reglage.fps && economie === reglage.economie) return;
  reglage = { fps, economie };
  ecouteurs.forEach((fn) => fn(reglage));
}

/** Appelle `fn` à chaque changement du réglage ; renvoie de quoi se désabonner. */
export function suivreReglageAnimations(fn: (r: ReglageAnimations) => void): () => void {
  ecouteurs.add(fn);
  return () => ecouteurs.delete(fn);
}

let charge = false;
/** Lit le réglage dans la coque, puis suit ses changements (une fois par fenêtre). */
export function chargerReglageAnimations(): void {
  if (charge || !disponible()) return;
  charge = true;
  // Version de Kobi sans ce réglage (ou coque simulée) : les défauts restent.
  invoke<ReglageAnimations | null>('reglage_animations').then((r) => r && poserReglageAnimations(r), () => {});
  ecouter<{ key: string; value: unknown }>('reglage', ({ key, value }) => {
    if (key === 'animations.fps') poserReglageAnimations({ fps: value as Fps });
    else if (key === 'animations.economie') poserReglageAnimations({ economie: value as boolean });
  });
}

/** Fréquences d'écran courantes : la mesure est arrondie à la plus proche. */
const FREQUENCES = [30, 48, 50, 60, 72, 75, 90, 100, 120, 144, 165, 240];
let ecran = 60;
let mesuree = false;
/** Fréquence imposée (tests, démo) : la mesure en cours ne la remplace pas. */
let imposee = false;

/** Fréquence de rafraîchissement de l'écran (mesurée au premier appel, 60 en attendant). */
export function frequenceEcran(): number {
  return ecran;
}

/** Prévenus quand la fréquence de l'écran est connue. */
const ecouteursEcran = new Set<() => void>();

/**
 * Mesure la fréquence de l'écran sur une vingtaine d'images (une fois), puis
 * prévient les abonnés. Hors écran (fenêtre cachée), garde 60.
 */
function mesurerEcran(): void {
  if (mesuree) return;
  mesuree = true;
  const temps: number[] = [];
  const image = (t: number): void => {
    temps.push(t);
    if (temps.length < 21) {
      requestAnimationFrame(image);
      return;
    }
    const ecarts = temps.slice(1).map((v, i) => v - temps[i]!).sort((a, b) => a - b);
    const median = ecarts[Math.floor(ecarts.length / 2)]!;
    if (median > 2 && median < 50 && !imposee) {
      const hz = 1000 / median;
      ecran = FREQUENCES.reduce((a, b) => (Math.abs(b - hz) < Math.abs(a - hz) ? b : a));
    }
    ecouteursEcran.forEach((fn) => fn());
  };
  requestAnimationFrame(image);
}

/**
 * Images par seconde voulues pour ce réglage : « suivre l'écran » prend sa
 * fréquence ; une valeur plus haute que l'écran ne peut pas être tenue.
 */
export function cadenceVoulue(r: ReglageAnimations = reglage, hz = ecran): number {
  return r.fps === 'ecran' ? hz : Math.min(r.fps, hz);
}

/**
 * Suit le réglage pour une fenêtre : `fn(cadence, voulue, reglage)` reçoit la
 * cadence à imposer au métronome (0 : celle de l'écran, le navigateur anime
 * lui-même) et les images par seconde voulues. Appelée tout de suite, puis à
 * chaque changement du réglage et après la mesure de l'écran.
 */
export function suivreCadence(fn: (cadence: number, voulue: number, r: ReglageAnimations) => void): () => void {
  chargerReglageAnimations();
  const appliquer = (): void => {
    const voulue = cadenceVoulue();
    fn(voulue >= frequenceEcran() ? 0 : voulue, voulue, reglage);
  };
  appliquer();
  ecouteursEcran.add(appliquer);
  mesurerEcran();
  const fin = suivreReglageAnimations(appliquer);
  return () => {
    fin();
    ecouteursEcran.delete(appliquer);
  };
}

/** Accès pour les tests et la démo : réglage et fréquence d'écran simulée. */
declare global {
  interface Window {
    kobiAnimations: { poser(r: Partial<ReglageAnimations>): void; ecran(hz: number): void };
  }
}
if (typeof window !== 'undefined') {
  window.kobiAnimations = {
    poser: poserReglageAnimations,
    ecran(hz: number): void {
      ecran = hz;
      mesuree = true;
      imposee = true;
      ecouteursEcran.forEach((fn) => fn());
    },
  };
}
