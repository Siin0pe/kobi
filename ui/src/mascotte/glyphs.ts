// Brouillage des caractères et clignements, signature de Kobi.
//
// À chaque changement d'état, les yeux et le mot passent par des symboles
// (`#%&@*?!/<>{}=+$`) : les yeux pendant 300 ms, puis le mot se pose lettre
// par lettre. Rythme du prototype : un pas toutes les 60 ms, la lettre i se
// pose au pas 1,6 × i + 3. La boucle ne tourne que pendant le brouillage.
// Les symboles tressautent, chaque lettre qui se pose rebondit. Tout passe par
// la boucle d'animation partagée (ticker.ts).

import { onFrame } from '../ticker';

export const NOISE = '#%&@*?!/<>{}=+$';
const STEP_MS = 60;
const EYE_STEPS = 5;
const LETTER_SPEED = 1.6;
const LETTER_DELAY = 3;

/** Caractère de bruit déterministe (même formule que le prototype). */
function noise(seed: number): string {
  return NOISE.charAt(((seed % NOISE.length) + NOISE.length) % NOISE.length);
}

/** Texte du mot au pas k : lettres posées ou symboles. */
export function scramble(target: string[], k: number): string[] {
  return target.map((c, i) => (c === ' ' || k > i * LETTER_SPEED + LETTER_DELAY ? c : noise(i * 7 + k * 3)));
}

/** Pas à partir duquel tout le mot est posé. */
export function settleStep(length: number): number {
  return Math.floor((length - 1) * LETTER_SPEED + LETTER_DELAY) + 1;
}

/** Rebond d'une lettre qui se pose (transformation seule : la graisse ondule à part). */
const LETTER_POP: Keyframe[] = [
  { transform: 'translateY(14px) scale(.6,1.3)', easing: 'cubic-bezier(.2,.8,.3,1.2)' },
  { offset: 0.45, transform: 'translateY(-10px) scale(1.12,.92)', easing: 'cubic-bezier(.45,0,.35,1)' },
  { offset: 0.75, transform: 'translateY(2px) scale(.97,1.03)' },
  { transform: 'none' },
];

/** Paire d'yeux : gauche, droite. */
export type Eyes = readonly [string, string];

/**
 * Écrit une lettre du mot. Son caractère est aussi dans `data-c` : les deux
 * couches de graisse de l'ondulation (kobi.css, KOB-200) le reprennent.
 */
function poser(span: HTMLElement, c: string): void {
  span.textContent = c;
  span.dataset.c = c;
}

export class Scrambler {
  private eye = '';
  private letters: string[] = [];
  private start = 0;
  private unsub: (() => void) | null = null;
  private lastStep = -1;
  private instant = false;
  /** Clignement en cours : les yeux affichent `–`. */
  private blinking = false;
  /** Expression passagère (clin d'œil, regard d'un œil…), sinon l'œil de l'état. */
  private expression: Eyes | null = null;
  private glitched: { span: HTMLElement; final: string; unsub: () => void } | null = null;

  constructor(
    private readonly eyes: HTMLElement[],
    private readonly word: HTMLElement,
  ) {}

  /** Pose une nouvelle expression, en brouillant si les animations sont permises. */
  show(eye: string, word: string): void {
    this.eye = eye;
    this.letters = Array.from(word);
    this.blinking = false;
    this.expression = null;
    const spans = this.letters.map((_, i) => {
      const span = document.createElement('span');
      span.className = 'ch';
      // Décalage par lettre, repris par les couches de l'ondulation.
      span.style.setProperty('--d', `${(i * 0.12).toFixed(2)}s`);
      return span;
    });
    // Une seule boucle à la fois : un changement d'état rapproché ne doit pas
    // laisser tourner l'ancienne (elle échapperait à la pause).
    this.stop();
    this.word.replaceChildren(...spans);
    this.start = performance.now();
    this.lastStep = -1;
    if (this.instant) {
      this.paint(Number.POSITIVE_INFINITY);
      return;
    }
    this.paint(0);
    this.loop();
  }

  setInstant(instant: boolean): void {
    this.instant = instant;
    if (instant) {
      this.stop();
      this.expression = null;
      this.paint(Number.POSITIVE_INFINITY);
    }
  }

  get scrambling(): boolean {
    return this.unsub !== null;
  }

  /**
   * Yeux posés : hors brouillage, ou quand seul le mot se pose encore (après
   * les 5 pas des yeux). Un clignement ou une expression s'y peint tout de
   * suite, sans attendre le pas suivant (KOB-401).
   */
  private get eyesSettled(): boolean {
    return !this.scrambling || this.lastStep >= EYE_STEPS;
  }

  /** Ferme ou rouvre les yeux (sans effet pendant le brouillage des yeux). */
  blink(closed: boolean): void {
    this.blinking = closed;
    if (this.eyesSettled) this.paintEyes();
  }

  /** Expression passagère des deux yeux, `null` pour revenir à celle de l'état. */
  express(eyes: Eyes | null): void {
    this.expression = eyes;
    if (this.eyesSettled) this.paintEyes();
  }

  /** Brouille une seule lettre du mot quelques pas, puis la repose (veille). */
  glitch(index: number): void {
    const span = this.word.children[index] as HTMLElement | undefined;
    const final = this.letters[index];
    if (!span || final === undefined || final === ' ' || this.scrambling || this.instant || this.glitched) return;
    const t0 = performance.now();
    let step = -1;
    const unsub = onFrame((now) => {
      const k = Math.floor((now - t0) / STEP_MS);
      if (k === step) return true;
      step = k;
      if (k < 4) {
        poser(span, noise(index * 5 + k * 3 + 1));
        return true;
      }
      this.endGlitch();
      span.animate(LETTER_POP, { duration: 360 });
      return false;
    });
    this.glitched = { span, final, unsub };
  }

  /** Pose tout de suite le mot et les yeux (pause en plein brouillage). */
  finish(): void {
    const running = this.unsub !== null;
    this.stop();
    if (running) this.paint(Number.POSITIVE_INFINITY);
  }

  /** Arrête la boucle de brouillage et repose une lettre brouillée seule. */
  stop(): void {
    this.unsub?.();
    this.unsub = null;
    this.endGlitch();
  }

  /**
   * Garde-fou (KOB-171) : hors brouillage, les yeux et le mot affichent
   * exactement l'expression courante et les lettres de l'état. Appelé après
   * chaque changement d'état, pause ou réglage.
   */
  resync(): void {
    if (this.scrambling) return;
    this.paintEyes();
    const spans = this.word.children;
    for (let i = 0; i < spans.length; i++) {
      const c = this.letters[i];
      if (c !== undefined && spans[i]!.textContent !== c) poser(spans[i] as HTMLElement, c);
    }
  }

  private endGlitch(): void {
    const g = this.glitched;
    this.glitched = null;
    if (!g) return;
    g.unsub();
    poser(g.span, g.final);
  }

  private loop(): void {
    this.unsub = onFrame((now) => {
      const k = Math.floor((now - this.start) / STEP_MS);
      if (k !== this.lastStep) this.paint(k);
      if (k < Math.max(EYE_STEPS, settleStep(this.letters.length))) return true;
      this.unsub = null;
      this.paintEyes();
      return false;
    });
  }

  private paint(k: number): void {
    this.lastStep = k;
    if (k < EYE_STEPS) {
      const n = noise(k * 5 + 2);
      for (const node of this.eyes) if (node.textContent !== n) node.textContent = n;
    } else {
      this.paintEyes();
    }
    const text = scramble(this.letters, k);
    const spans = this.word.children;
    const live = Number.isFinite(k);
    for (let i = 0; i < spans.length; i++) {
      const span = spans[i] as HTMLElement;
      const c = text[i]!;
      if (span.textContent === c) continue;
      const settled = c === this.letters[i];
      poser(span, c);
      if (!live) continue;
      if (settled) {
        span.style.transform = '';
        if (c !== ' ') span.animate(LETTER_POP, { duration: 380 });
      } else {
        // Le symbole tressaute : petit décalage pseudo-aléatoire à chaque pas.
        const s = (i * 13 + k * 7) % 9;
        span.style.transform = `translateY(${s - 4}px) rotate(${(s % 5) * 4 - 8}deg)`;
      }
    }
  }

  private paintEyes(): void {
    const pair: Eyes = this.blinking ? ['–', '–'] : (this.expression ?? [this.eye, this.eye]);
    this.eyes.forEach((node, i) => {
      if (node.textContent !== pair[i]) node.textContent = pair[i]!;
    });
  }
}

/**
 * Clignements aléatoires : yeux fermés 120 ms toutes les 2 à 5 s, parfois
 * deux fois de suite. Une seule minuterie à la fois, aucune quand la fenêtre
 * est masquée ou les animations réduites.
 */
export class Blinker {
  private timer = 0;
  private active = false;

  constructor(
    private readonly onBlink: (closed: boolean) => void,
    private readonly random: () => number = Math.random,
  ) {}

  run(active: boolean): void {
    if (active === this.active) return;
    this.active = active;
    window.clearTimeout(this.timer);
    this.timer = 0;
    if (active) this.next(800);
    else this.onBlink(false);
  }

  private next(minDelay = 2000): void {
    const delay = minDelay + this.random() * 3000;
    this.timer = window.setTimeout(() => this.blink(this.random() < 0.15 ? 2 : 1), delay);
  }

  private blink(times: number): void {
    this.onBlink(true);
    this.timer = window.setTimeout(() => {
      this.onBlink(false);
      if (!this.active) return;
      if (times > 1) this.timer = window.setTimeout(() => this.blink(times - 1), 140);
      else this.next();
    }, 120);
  }
}
