// Brouillage des caractères et clignements, signature du skin Kobi.
//
// À chaque changement d'état, les yeux et le mot passent par des symboles
// (`#%&@*?!/<>{}=+$`) : les yeux pendant 300 ms, puis le mot se pose lettre
// par lettre. Rythme du prototype : un pas toutes les 60 ms, la lettre i se
// pose au pas 1,6 × i + 3. La boucle ne tourne que pendant le brouillage.

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

export class Scrambler {
  private eye = '';
  private letters: string[] = [];
  private start = 0;
  private frame = 0;
  private lastStep = -1;
  private instant = false;
  /** Clignement en cours : les yeux affichent `–`. */
  private blinking = false;

  constructor(
    private readonly eyes: HTMLElement[],
    private readonly word: HTMLElement,
  ) {}

  /** Pose une nouvelle expression, en brouillant si les animations sont permises. */
  show(eye: string, word: string): void {
    this.eye = eye;
    this.letters = Array.from(word);
    this.blinking = false;
    const spans = this.letters.map((_, i) => {
      const span = document.createElement('span');
      span.className = 'ch';
      span.style.animationDelay = `${(i * 0.12).toFixed(2)}s`;
      return span;
    });
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
      this.paint(Number.POSITIVE_INFINITY);
    }
  }

  get scrambling(): boolean {
    return this.frame !== 0;
  }

  /** Ferme ou rouvre les yeux (sans effet pendant le brouillage). */
  blink(closed: boolean): void {
    this.blinking = closed;
    if (!this.scrambling) this.paintEyes(closed ? '–' : this.eye);
  }

  stop(): void {
    cancelAnimationFrame(this.frame);
    this.frame = 0;
  }

  private loop(): void {
    this.frame = requestAnimationFrame((now) => {
      this.frame = 0;
      const k = Math.floor((now - this.start) / STEP_MS);
      if (k !== this.lastStep) this.paint(k);
      if (k < Math.max(EYE_STEPS, settleStep(this.letters.length))) this.loop();
    });
  }

  private paint(k: number): void {
    this.lastStep = k;
    const eye = k < EYE_STEPS ? noise(k * 5 + 2) : this.blinking ? '–' : this.eye;
    this.paintEyes(eye);
    const text = scramble(this.letters, k);
    const spans = this.word.children;
    for (let i = 0; i < spans.length; i++) {
      const span = spans[i]!;
      if (span.textContent !== text[i]) span.textContent = text[i]!;
    }
  }

  private paintEyes(eye: string): void {
    for (const node of this.eyes) if (node.textContent !== eye) node.textContent = eye;
  }
}

/**
 * Clignements aléatoires (Écoute, Note, Répond) : yeux fermés 120 ms toutes
 * les 2 à 5 s, parfois deux fois de suite. Une seule minuterie à la fois,
 * aucune quand la fenêtre est masquée ou les animations réduites.
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
