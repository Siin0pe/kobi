// Scène : héberge un skin, les bulles et cartes communes et la région lue par
// les lecteurs d'écran. Elle applique les réglages système (réduire les
// animations) et met tout en pause quand la fenêtre est masquée.

import { Bubbles, type ActionDecision } from './bubbles';
import {
  STATE_LABELS,
  STATE_SPOKEN,
  type MascotData,
  type MascotState,
  type Skin,
  type SkinFactory,
  type Theme,
} from './contract';

/** Taille logique de la scène, en px ; elle est mise à l'échelle de la fenêtre. */
export const SCENE_WIDTH = 820;
export const SCENE_HEIGHT = 560;

export interface StageOptions {
  /** Affiche le fond du thème (captures, développement). La fenêtre native reste transparente. */
  backdrop?: boolean;
  /** Force « réduire les animations » (tests) ; sinon suit le réglage système. */
  reducedMotion?: boolean;
}

export class Stage {
  readonly root: HTMLElement;
  private readonly scene: HTMLElement;
  private readonly skinHost: HTMLElement;
  private readonly live: HTMLElement;
  private readonly bubbles: Bubbles;
  private skin: Skin | null = null;
  private state: MascotState = 'idle';
  private data: MascotData = {};
  private spoken: MascotState | null = null;
  private paused = false;
  private reducedMotion: boolean;
  private readonly motionQuery = window.matchMedia('(prefers-reduced-motion: reduce)');
  private readonly forcedMotion: boolean | undefined;
  private readonly cleanups: Array<() => void> = [];
  /** Animations arrêtées par la mise en pause, à relancer au retour. */
  private frozen: Animation[] = [];

  constructor(parent: HTMLElement, options: StageOptions = {}) {
    this.forcedMotion = options.reducedMotion;
    this.reducedMotion = options.reducedMotion ?? this.motionQuery.matches;

    this.root = document.createElement('div');
    this.root.className = 'stage';
    this.root.classList.toggle('with-backdrop', options.backdrop ?? false);

    this.scene = document.createElement('div');
    this.scene.className = 'scene';
    this.scene.setAttribute('role', 'img');

    this.skinHost = document.createElement('div');
    this.skinHost.className = 'skin-host';
    this.skinHost.setAttribute('aria-hidden', 'true');

    this.bubbles = new Bubbles();

    this.live = document.createElement('div');
    this.live.className = 'sr-only';
    this.live.setAttribute('role', 'status');
    this.live.setAttribute('aria-live', 'polite');

    this.scene.append(this.skinHost, this.bubbles.root);
    this.root.append(this.scene, this.live);
    parent.append(this.root);

    this.fit();
    const onResize = (): void => this.fit();
    window.addEventListener('resize', onResize);
    this.cleanups.push(() => window.removeEventListener('resize', onResize));

    const onVisibility = (): void => this.applyPaused();
    document.addEventListener('visibilitychange', onVisibility);
    this.cleanups.push(() => document.removeEventListener('visibilitychange', onVisibility));

    if (this.forcedMotion === undefined) {
      const onMotion = (e: MediaQueryListEvent): void => this.setReducedMotion(e.matches);
      this.motionQuery.addEventListener('change', onMotion);
      this.cleanups.push(() => this.motionQuery.removeEventListener('change', onMotion));
    }
    this.root.classList.toggle('reduced-motion', this.reducedMotion);
    this.bubbles.setReducedMotion(this.reducedMotion);
    this.setState('idle');
  }

  /** Remplace le skin courant ; le nouveau reprend l'état et les données. */
  useSkin(factory: SkinFactory): Skin {
    this.skin?.destroy();
    this.skinHost.replaceChildren();
    const skin = factory();
    this.skin = skin;
    this.applyTheme(skin.theme);
    this.root.dataset.skin = skin.id;
    skin.mount(this.skinHost, { reducedMotion: this.reducedMotion });
    skin.setState(this.state, this.data);
    if (this.isPaused()) skin.setPaused(true);
    this.paused = this.isPaused();
    return skin;
  }

  get currentState(): MascotState {
    return this.state;
  }

  /** Change d'état. Les données remplacent les précédentes. */
  setState(state: MascotState, data: MascotData = {}): void {
    this.state = state;
    this.data = { ...data };
    this.root.dataset.state = state;
    this.scene.setAttribute('aria-label', `Kobi : ${STATE_LABELS[state]}`);
    this.skin?.setState(state, this.data);
    this.bubbles.render(state, this.data);
    if (state !== this.spoken) {
      this.spoken = state;
      this.live.textContent = STATE_SPOKEN[state];
    }
  }

  /** Met à jour les données sans changer d'état (texte en flux, sources). */
  update(data: MascotData): void {
    this.data = { ...this.data, ...data };
    this.skin?.setData(this.data);
    this.bubbles.render(this.state, this.data);
  }

  /** Appelé quand l'utilisateur confirme ou refuse une carte d'action. */
  onAction(handler: (decision: ActionDecision) => void): void {
    this.bubbles.onAction = handler;
  }

  setReducedMotion(reduced: boolean): void {
    this.reducedMotion = reduced;
    this.root.classList.toggle('reduced-motion', reduced);
    this.bubbles.setReducedMotion(reduced);
    this.skin?.setReducedMotion(reduced);
  }

  destroy(): void {
    this.skin?.destroy();
    this.cleanups.forEach((fn) => fn());
    this.root.remove();
  }

  private isPaused(): boolean {
    return document.visibilityState === 'hidden';
  }

  private applyPaused(): void {
    const paused = this.isPaused();
    if (paused === this.paused) return;
    this.paused = paused;
    this.root.classList.toggle('paused', paused);
    if (paused) {
      // Tout ce qui tourne s'arrête : animations CSS, transitions en vol,
      // animations Web des skins.
      this.frozen = this.root.getAnimations({ subtree: true }).filter((a) => a.playState === 'running');
      this.frozen.forEach((a) => a.pause());
    } else {
      this.frozen.forEach((a) => {
        if (a.playState === 'paused') a.play();
      });
      this.frozen = [];
    }
    this.bubbles.setPaused(paused);
    this.skin?.setPaused(paused);
  }

  private applyTheme(theme: Theme): void {
    const s = this.root.style;
    s.setProperty('--backdrop', theme.backdrop);
    s.setProperty('--ink', theme.ink);
    s.setProperty('--paper', theme.paper);
    s.setProperty('--accent', theme.accent);
    s.setProperty('--muted', theme.muted);
    s.setProperty('--danger', theme.danger);
    s.setProperty('--font', theme.font);
  }

  private fit(): void {
    const scale = Math.min(window.innerWidth / SCENE_WIDTH, window.innerHeight / SCENE_HEIGHT);
    this.root.style.setProperty('--scale', String(scale));
  }
}
