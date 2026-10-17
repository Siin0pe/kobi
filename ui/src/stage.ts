// Scène : héberge un skin, les bulles et cartes communes et la région lue par
// les lecteurs d'écran. Elle applique les réglages système (réduire les
// animations) et met tout en pause quand la fenêtre est masquée.

import { Bubbles, type ActionDecision } from './bubbles';
import {
  DOT_COLORS,
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
  private readonly dot: HTMLButtonElement;
  private compact = false;
  /** Appelé quand l'utilisateur déplie la pastille (la coque peut agrandir la fenêtre). */
  onCompactChange: ((compact: boolean) => void) | null = null;
  private skin: Skin | null = null;
  private state: MascotState = 'idle';
  private data: MascotData = {};
  private spoken: MascotState | null = null;
  private paused = false;
  private windowHidden = false;
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

    // Mode discret : une pastille de 32 px colorée selon l'état ; un clic la déplie.
    this.dot = document.createElement('button');
    this.dot.type = 'button';
    this.dot.className = 'dot';
    this.dot.hidden = true;
    this.dot.addEventListener('click', () => this.setCompact(false));

    this.scene.append(this.skinHost, this.bubbles.root);
    this.root.append(this.scene, this.dot, this.live);
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
    this.paintDot();
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
    this.paintDot();
    this.skin?.setState(state, this.data);
    this.bubbles.render(state, this.data);
    if (state !== this.spoken) {
      this.spoken = state;
      this.live.textContent = STATE_SPOKEN[state];
    }
  }

  get isCompact(): boolean {
    return this.compact;
  }

  /**
   * Mode discret : la mascotte et les cartes laissent place à la pastille.
   * Le skin est mis en pause tant qu'il est caché (aucun coût CPU).
   */
  setCompact(compact: boolean): void {
    if (compact === this.compact) return;
    this.compact = compact;
    this.root.classList.toggle('compact', compact);
    this.scene.hidden = compact;
    this.dot.hidden = !compact;
    this.applyPaused();
    this.onCompactChange?.(compact);
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

  /** Pause : fenêtre masquée, ou mascotte repliée en pastille. */
  private isPaused(): boolean {
    return document.visibilityState === 'hidden' || this.compact;
  }

  private paintDot(): void {
    const color = this.skin?.theme.dots?.[this.state] ?? DOT_COLORS[this.state];
    this.dot.style.setProperty('--dot', color);
    this.dot.dataset.state = this.state;
    this.dot.setAttribute('aria-label', `Kobi : ${STATE_LABELS[this.state]}. Déplier la mascotte`);
    this.dot.title = `Kobi : ${STATE_LABELS[this.state]}`;
  }

  private applyPaused(): void {
    const paused = this.isPaused();
    const hidden = document.visibilityState === 'hidden';
    if (paused === this.paused && hidden === this.windowHidden) return;
    this.paused = paused;
    this.windowHidden = hidden;
    // `paused` arrête la scène (mascotte et cartes) ; `window-hidden` arrête
    // tout, pastille comprise.
    this.root.classList.toggle('paused', paused);
    this.root.classList.toggle('window-hidden', hidden);
    this.frozen.forEach((a) => {
      if (a.playState === 'paused') a.play();
    });
    this.frozen = [];
    const scope = hidden ? this.root : paused ? this.scene : null;
    if (scope) {
      // Animations CSS, transitions en vol, animations Web des skins.
      this.frozen = scope.getAnimations({ subtree: true }).filter((a) => a.playState === 'running');
      this.frozen.forEach((a) => a.pause());
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
