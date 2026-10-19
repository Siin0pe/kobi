// Images par seconde (KOB-200), et repos économe (KOB-146) : un métronome
// pour toute la scène.
//
// Par défaut, tout bouge à 60 images/s (réglage « Images par seconde » de
// l'app : 5, 15, 30, 60, 120 ou suivre l'écran, cadence.ts). Sous la fréquence
// de l'écran, les boucles CSS infinies et les animations Web sont mises en
// pause et une seule horloge les fait avancer ensemble, au rythme réglé :
// toutes changent sur la même image, quelle que soit leur courbe. À la
// fréquence de l'écran (ou au-dessus), le navigateur les anime lui-même.
//
// « Économie d'énergie » (désactivée par défaut) reprend l'ancien repos
// économe : au repos prolongé, 3 images/s, puis 2 après une minute. Elle ne
// fait que plafonner la cadence réglée.
//
// Les gestes de la vie au repos (lettre qui sautille, bâillement, regard) sont
// des animations Web lancées par `el.animate()` : elles suivent aussi le
// métronome, puis sont relâchées à leur fin (leur `onfinish` se déclenche).
// La boucle partagée (ticker.ts : lettres qui se brouillent, frappe des
// bulles) prend le même rythme, et le métronome avance sur ses images.

import { onFrame, setFrameInterval } from './ticker';

/** Images par seconde de l'économie d'énergie au repos, puis au sommeil prolongé. */
export const ECONOMY_FPS = 3;
export const DEEP_FPS = 2;
/** Délai avant le sommeil prolongé. */
export const DEEP_AFTER_MS = 60_000;

/** Métronomes actifs : ils adoptent les animations Web créées dans leur scène. */
const active = new Set<Metronome>();
const nativeAnimate = Element.prototype.animate;
Element.prototype.animate = function (this: Element, ...args: Parameters<Element['animate']>): Animation {
  const anim = nativeAnimate.apply(this, args);
  for (const m of active) m.adopt(this, anim);
  return anim;
};

function isLoop(a: Animation): boolean {
  return a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity;
}

export class Metronome {
  /** Animations tenues par le métronome, avec l'instant où elles ont été avancées. */
  private readonly held = new Map<Animation, number>();
  private unsub: (() => void) | null = null;
  private slow = 0;
  /** Cadence réglée, 0 : celle de l'écran (le navigateur anime lui-même). */
  private cadence = 0;
  /** Repos économe (« Économie d'énergie » au repos prolongé). */
  private economie = false;
  /** Geste en cours (scène de la veille, ballottement) : voir `setLively`. */
  private lively = false;
  private suspended = false;
  private fps = ECONOMY_FPS;
  private readonly onStart = (e: Event): void => {
    if (!this.rate) return;
    const target = e.target as Element;
    // Une boucle qui démarre (fiche, « z ») suit le métronome.
    for (const a of target.getAnimations()) if (isLoop(a) && a.playState === 'running') this.hold(a);
  };

  /** @param root élément dont les boucles sont tenues ; `onRate` reçoit le rythme (tests). */
  constructor(
    private readonly root: HTMLElement,
    private readonly onRate?: (fps: number) => void,
  ) {
    root.addEventListener('animationstart', this.onStart);
  }

  /** Images par seconde imposées, 0 : celles de l'écran (le navigateur anime). */
  get rate(): number {
    if (!this.economie) return this.cadence;
    return this.cadence ? Math.min(this.cadence, this.fps) : this.fps;
  }

  /** Animation Web créée dans la scène : elle suit le métronome. */
  adopt(el: Element, anim: Animation): void {
    if (this.rate && !this.geste() && this.root.contains(el)) this.hold(anim);
  }

  /**
   * Images par seconde réglées (KOB-200), 0 pour celles de l'écran.
   * Appliqué à chaud : les boucles sont tenues ou relâchées sur-le-champ.
   */
  setCadence(fps: number): void {
    if (fps === this.cadence) return;
    this.cadence = fps;
    this.appliquer();
  }

  /** Entrée (ou sortie) du repos économe (« Économie d'énergie » activée). */
  setActive(on: boolean): void {
    if (on === this.economie) return;
    this.economie = on;
    window.clearTimeout(this.slow);
    this.fps = ECONOMY_FPS;
    if (on) this.slow = window.setTimeout(() => this.setRate(DEEP_FPS), DEEP_AFTER_MS);
    this.appliquer();
  }

  /** Fenêtre masquée ou scène repliée : plus aucune image, boucles tenues en l'état. */
  setSuspended(suspended: boolean): void {
    this.suspended = suspended;
    if (suspended) this.halt();
    else if (this.rate) this.run();
  }

  destroy(): void {
    this.setActive(false);
    this.setCadence(0);
    this.root.removeEventListener('animationstart', this.onStart);
  }

  /**
   * Geste au repos économe (scène de la veille, ballottement, KOB-149) : à
   * la cadence de l'écran, il se joue à pleine vitesse (ses animations Web ne
   * sont plus tenues, la boucle partagée tourne à chaque image), mais les
   * boucles de fond restent au rythme du repos économe (KOB-170).
   */
  setLively(on: boolean): void {
    if (on === this.lively) return;
    this.lively = on;
    this.frameInterval();
  }

  /** Geste à pleine vitesse pendant le repos économe, sur un écran laissé à lui-même. */
  private geste(): boolean {
    return this.lively && this.economie && this.cadence === 0;
  }

  /** Tient ou relâche tout selon le rythme courant. */
  private appliquer(): void {
    if (this.rate) {
      active.add(this);
      for (const a of this.root.getAnimations({ subtree: true })) if (isLoop(a) && a.playState === 'running') this.hold(a);
      this.run();
    } else {
      active.delete(this);
      this.halt();
      for (const a of this.held.keys()) if (a.playState === 'paused') a.play();
      this.held.clear();
    }
    this.frameInterval();
    this.onRate?.(this.rate);
  }

  /** La boucle partagée suit le métronome, sauf pendant un geste à pleine vitesse. */
  private frameInterval(): void {
    setFrameInterval(this.rate && !this.geste() ? 1000 / this.rate : 0);
  }

  private setRate(fps: number): void {
    this.fps = fps;
    this.frameInterval();
    this.onRate?.(this.rate);
  }

  private hold(a: Animation): void {
    a.pause();
    // Une animation adoptée entre deux images n'avance que du temps écoulé depuis.
    this.held.set(a, performance.now());
  }

  private run(): void {
    this.halt();
    if (this.suspended) return;
    // Reprise (après une suspension) : le temps passé caché ne compte pas.
    const now = performance.now();
    for (const a of this.held.keys()) this.held.set(a, now);
    // Une image de la boucle partagée (déjà au rythme réglé) : tout avance ensemble.
    this.unsub = onFrame(() => {
      this.step();
      return true;
    });
  }

  private halt(): void {
    this.unsub?.();
    this.unsub = null;
  }

  /** Une image : toutes les animations tenues avancent ensemble, chacune du temps écoulé pour elle. */
  private step(): void {
    const now = performance.now();
    for (const [a, since] of this.held) {
      const target = (a.effect as KeyframeEffect | null)?.target;
      // Animation remplacée (changement d'état) ou élément retiré : on la lâche.
      if (a.playState === 'idle' || !target?.isConnected) {
        this.held.delete(a);
        continue;
      }
      this.held.set(a, now);
      // Relancée par ailleurs (changement de classe) : le métronome la reprend.
      if (a.playState !== 'paused') {
        if (a.playState === 'running') a.pause();
        continue;
      }
      const next = Number(a.currentTime ?? 0) + (now - since);
      const end = Number(a.effect!.getComputedTiming().endTime);
      if (next >= end) {
        // Fin d'une animation courte : posée sur sa dernière image et relâchée,
        // elle se termine normalement (onfinish, fill).
        a.currentTime = end;
        a.play();
        this.held.delete(a);
      } else a.currentTime = next;
    }
  }
}
