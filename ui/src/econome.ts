// Repos économe (KOB-146) : un métronome pour toute la scène.
//
// Au repos, rien ne s'arrête (« toujours animé, même le texte »), mais sans
// GPU chaque image coûte : WebKitGTK repeint et recompose la fenêtre. Les
// boucles CSS infinies sont donc mises en pause et une seule minuterie les fait
// avancer ensemble : toutes les animations changent sur la même image, au
// rythme choisi (3 images/s, puis 2 après une minute), quelle que soit leur
// courbe. Mesuré dans l'app (WebKitGTK sans GPU, KOB-146) : le coût d'une
// image varie peu avec ce qui bouge, c'est leur nombre qui compte.
//
// Les gestes de la vie au repos (lettre qui sautille, bâillement, regard) sont
// des animations Web lancées par `el.animate()` : elles suivent aussi le
// métronome, puis sont relâchées à leur fin (leur `onfinish` se déclenche).
// La boucle partagée (ticker.ts : lettres qui se brouillent) prend le même
// rythme.

import { setFrameInterval } from './ticker';

/** Images par seconde du repos économe, puis du sommeil prolongé. */
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
  private tick = 0;
  private slow = 0;
  private active = false;
  /** Geste en cours (scène de la veille, ballottement) : voir `setLively`. */
  private lively = false;
  private suspended = false;
  private fps = ECONOMY_FPS;
  private readonly onStart = (e: Event): void => {
    if (!this.active) return;
    const target = e.target as Element;
    // Une boucle qui démarre pendant le repos (fiche, « z ») suit le métronome.
    for (const a of target.getAnimations()) if (isLoop(a) && a.playState === 'running') this.hold(a);
  };

  /** @param root élément dont les boucles sont tenues ; `onRate` reçoit le rythme (tests). */
  constructor(
    private readonly root: HTMLElement,
    private readonly onRate?: (fps: number) => void,
  ) {
    root.addEventListener('animationstart', this.onStart);
  }

  get rate(): number {
    return this.active ? this.fps : 0;
  }

  /** Animation Web créée dans la scène pendant le repos : elle suit le métronome. */
  adopt(el: Element, anim: Animation): void {
    if (this.active && !this.lively && this.root.contains(el)) this.hold(anim);
  }

  /** Entrée (ou sortie) du repos économe. */
  setActive(on: boolean): void {
    if (on === this.active) return;
    this.active = on;
    window.clearTimeout(this.slow);
    if (on) {
      active.add(this);
      this.fps = ECONOMY_FPS;
      for (const a of this.root.getAnimations({ subtree: true })) if (isLoop(a) && a.playState === 'running') this.hold(a);
      this.slow = window.setTimeout(() => this.setRate(DEEP_FPS), DEEP_AFTER_MS);
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

  /** Fenêtre masquée ou scène repliée : plus aucune image, boucles tenues en l'état. */
  setSuspended(suspended: boolean): void {
    this.suspended = suspended;
    if (suspended) this.halt();
    else if (this.active) this.run();
  }

  destroy(): void {
    this.setActive(false);
    this.root.removeEventListener('animationstart', this.onStart);
  }

  /**
   * Geste au repos (scène de la veille, ballottement, KOB-149) : il se joue à
   * pleine vitesse (ses animations Web ne sont plus tenues, la boucle
   * partagée tourne à chaque image), mais les boucles de fond restent au
   * rythme du métronome. Relâcher tout le métronome faisait repartir à
   * 60 images/s l'ondulation de la police, qui refait la mise en page à chaque
   * image : trois fois plus de calcul au repos pour le même mouvement (KOB-170).
   */
  setLively(on: boolean): void {
    if (on === this.lively) return;
    this.lively = on;
    this.frameInterval();
  }

  /** La boucle partagée suit le métronome, sauf pendant un geste. */
  private frameInterval(): void {
    setFrameInterval(this.active && !this.lively ? 1000 / this.fps : 0);
  }

  private setRate(fps: number): void {
    this.fps = fps;
    if (this.tick) this.run();
    this.frameInterval();
    this.onRate?.(this.rate);
  }

  private hold(a: Animation): void {
    a.pause();
    // Une animation adoptée entre deux tics n'avance que du temps écoulé depuis.
    this.held.set(a, performance.now());
  }

  private run(): void {
    this.halt();
    if (this.suspended) return;
    // Reprise (après une suspension) : le temps passé caché ne compte pas.
    const now = performance.now();
    for (const a of this.held.keys()) this.held.set(a, now);
    this.tick = window.setInterval(() => this.step(), 1000 / this.fps);
  }

  private halt(): void {
    window.clearInterval(this.tick);
    this.tick = 0;
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
