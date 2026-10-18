// Repos économe (KOB-146) : un métronome pour toute la scène.
//
// Au repos, rien ne s'arrête (« toujours animé, même le texte »), mais sans
// GPU chaque image coûte : WebKitGTK repeint et recompose la fenêtre. Les
// boucles CSS infinies sont donc mises en pause et une seule minuterie les fait
// avancer ensemble : toutes les animations changent sur la même image, au
// rythme choisi (4 images/s, puis 2 après deux minutes), quelle que soit leur
// courbe. Les animations courtes (arrivées, gestes, lettres qui sautillent)
// tournent normalement : elles sont rares et finissent vite.

/** Images par seconde du repos économe, puis du sommeil prolongé. */
export const ECONOMY_FPS = 4;
export const DEEP_FPS = 2;
/** Délai avant le sommeil prolongé. */
export const DEEP_AFTER_MS = 120_000;

function isLoop(a: Animation): boolean {
  return a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity;
}

export class Metronome {
  /** Boucles tenues par le métronome. */
  private readonly held = new Set<Animation>();
  private tick = 0;
  private slow = 0;
  private last = 0;
  private active = false;
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

  /** Entrée (ou sortie) du repos économe. */
  setActive(active: boolean): void {
    if (active === this.active) return;
    this.active = active;
    window.clearTimeout(this.slow);
    if (active) {
      this.fps = ECONOMY_FPS;
      for (const a of this.root.getAnimations({ subtree: true })) if (isLoop(a) && a.playState === 'running') this.hold(a);
      this.slow = window.setTimeout(() => this.setRate(DEEP_FPS), DEEP_AFTER_MS);
      this.run();
    } else {
      this.halt();
      for (const a of this.held) if (a.playState === 'paused') a.play();
      this.held.clear();
    }
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

  private setRate(fps: number): void {
    this.fps = fps;
    if (this.tick) this.run();
    this.onRate?.(this.rate);
  }

  private hold(a: Animation): void {
    a.pause();
    this.held.add(a);
  }

  private run(): void {
    this.halt();
    if (this.suspended) return;
    this.last = performance.now();
    this.tick = window.setInterval(() => this.step(), 1000 / this.fps);
  }

  private halt(): void {
    window.clearInterval(this.tick);
    this.tick = 0;
  }

  /** Une image : toutes les boucles avancent du temps écoulé, ensemble. */
  private step(): void {
    const now = performance.now();
    const dt = now - this.last;
    this.last = now;
    for (const a of this.held) {
      const target = (a.effect as KeyframeEffect | null)?.target;
      // Animation remplacée (changement d'état) ou élément retiré : on la lâche.
      if (a.playState === 'idle' || !target?.isConnected) {
        this.held.delete(a);
        continue;
      }
      if (a.playState === 'paused') a.currentTime = Number(a.currentTime ?? 0) + dt;
    }
  }
}
