// Kobi, style Glyphe : un visage fait de lettres. Port du prototype
// docs/design/mascotte/Glyphe.dc.html ; le CSS et les keyframes sont repris,
// la logique (horloge de 60 ms qui redessinait tout) est réécrite : seules
// les animations de l'état courant tournent, la plupart dans le moteur CSS.

import type { MascotState } from '../states';
import { Blinker, Scrambler } from './glyphs';
import { Inertia } from './motion';
import { Orbit } from './orbit';
import './kobi.css';

interface Look {
  eye: string;
  word: string;
}

export const LOOKS: Record<MascotState, Look> = {
  idle: { eye: '–', word: 'zzz' },
  listen: { eye: 'O', word: 'oui ?' },
  think: { eye: '@', word: 'hmm…' },
  write: { eye: 'o', word: 'noté' },
  answer: { eye: '^', word: 'alors' },
  suggest: { eye: '*', word: 'idée !' },
  done: { eye: '^', word: 'fait !' },
  error: { eye: 'x', word: 'oups' },
};

/** États où Kobi cligne des yeux. */
const BLINKS: ReadonlySet<MascotState> = new Set(['listen', 'write', 'answer']);

/** Délai avant le sommeil profond (veille économe, voir kobi.css). */
export const DEEP_SLEEP_MS = 20_000;

/** Bouche qui parle (Répond), une image toutes les 120 ms comme le prototype. */
const MOUTH = ['o', 'O', 'o', '-', 'O', 'o'];
const MOUTH_MS = 120;

function el(tag: string, className?: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** La mascotte, posée par la scène dans la partie droite (x ≥ 400 px). */
export class Kobi {
  readonly root: HTMLElement;
  private readonly pill: HTMLElement;
  private readonly eyes: HTMLElement[];
  private readonly mouth: HTMLElement;
  private readonly word: HTMLElement;
  private orbit = new Orbit();
  private readonly inertia: Inertia;
  private readonly scrambler: Scrambler;
  private readonly blinker: Blinker;
  private state: MascotState | null = null;
  private reduced = false;
  private paused = false;
  private mouthTimer = 0;
  private deepTimer = 0;
  private mouthStep = 0;

  constructor(reducedMotion: boolean) {
    this.reduced = reducedMotion;
    this.root = el('div', 'kobi');
    this.root.setAttribute('aria-hidden', 'true');
    this.root.classList.toggle('reduced-motion', this.reduced);

    const body = el('div', 'body');
    const zz = el('div', 'fx zz');
    zz.style.cssText = 'right:6px;top:-10px';
    zz.append(el('span', '', 'z'), el('span', '', 'z'), el('span', '', 'z'));
    const waves = el('div', 'fx waves');
    waves.style.cssText = 'left:-64px;top:48px';
    waves.append(el('span', '', '('), el('span', '', '('), el('span', '', '('));
    const bang = el('div', 'fx bang', '!');
    bang.style.cssText = 'right:-78px;top:-30px';
    const drop = el('div', 'fx drop', ',');
    drop.style.cssText = 'right:36px;top:4px';

    const wrapper = el('div', 'inertia');
    this.pill = el('div', 'pill');
    const eyes = el('div', 'eyes');
    this.eyes = [el('span', 'g'), el('span', 'g')];
    eyes.append(...this.eyes);
    this.mouth = el('div', 'mouth');
    this.pill.append(eyes, this.mouth);
    wrapper.append(this.pill);
    this.inertia = new Inertia(wrapper, this.pill);

    body.append(this.orbit.root, zz, waves, bang, drop, wrapper);
    this.word = el('div', 'word');
    this.root.append(body, this.word);
    this.scrambler = new Scrambler(this.eyes, this.word);
    this.scrambler.setInstant(this.reduced);
    this.blinker = new Blinker((closed) => this.scrambler.blink(closed));
  }

  setState(state: MascotState): void {
    if (state === this.state) return;
    const previous = this.state;
    this.state = state;
    const look = LOOKS[state];

    this.inertia.carry(() => {
      this.root.className = `kobi m-${state}`;
      this.root.classList.toggle('reduced-motion', this.reduced);
      this.root.classList.toggle('paused', this.paused);
    }, previous !== null && !this.reduced);

    this.root.dataset.state = state;
    this.scrambler.show(look.eye, look.word);
    this.mouthStep = 0;
    this.mouth.textContent = state === 'error' ? '~' : state === 'answer' ? MOUTH[0]! : '';

    if (state === 'think') this.orbit.start(this.reduced);
    else this.orbit.stop();
    if (this.paused) this.orbit.pause(true);
    this.syncTimers();
    window.clearTimeout(this.deepTimer);
    if (state === 'idle') this.deepTimer = window.setTimeout(() => this.deepSleep(), DEEP_SLEEP_MS);
  }

  /** Passe la veille en sommeil profond, en posant le mot sans à-coup. */
  private deepSleep(): void {
    if (this.state !== 'idle') return;
    const letters = [...this.word.children] as HTMLElement[];
    const from = letters.map((l) => getComputedStyle(l).fontVariationSettings);
    // Les « z » changent d'animation : chacune reprend là où en était l'ancienne.
    const zs = [...this.root.querySelectorAll<HTMLElement>('.zz span')];
    const times = zs.map((z) => z.getAnimations()[0]?.currentTime ?? null);
    this.root.classList.add('deep');
    zs.forEach((z, i) => {
      const anim = z.getAnimations()[0];
      if (anim && times[i] !== null) anim.currentTime = times[i]!;
    });
    if (this.reduced) return;
    letters.forEach((l, i) => {
      const to = getComputedStyle(l).fontVariationSettings;
      l.animate([{ fontVariationSettings: from[i]! }, { fontVariationSettings: to }], { duration: 1200, easing: 'ease-in-out' });
    });
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    this.root.classList.toggle('paused', paused);
    this.orbit.pause(paused || this.reduced);
    this.inertia.pause(paused);
    this.syncTimers();
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
    this.root.classList.toggle('reduced-motion', reduced);
    if (reduced) this.inertia.cancel();
    this.orbit.pause(reduced || this.paused);
    this.scrambler.setInstant(reduced);
    this.syncTimers();
  }

  destroy(): void {
    window.clearInterval(this.mouthTimer);
    window.clearTimeout(this.deepTimer);
    this.blinker.run(false);
    this.scrambler.stop();
    this.orbit.stop();
    this.inertia.cancel();
    this.root.remove();
  }

  /**
   * Minuteries de l'état courant, seulement fenêtre visible et animations
   * permises : bouche qui parle (Répond) et clignements.
   */
  private syncTimers(): void {
    const live = !this.paused && !this.reduced;
    this.blinker.run(live && this.state !== null && BLINKS.has(this.state));
    const run = live && this.state === 'answer';
    if (run && !this.mouthTimer) {
      this.mouthTimer = window.setInterval(() => {
        this.mouthStep = (this.mouthStep + 1) % MOUTH.length;
        this.mouth.textContent = MOUTH[this.mouthStep]!;
      }, MOUTH_MS);
    } else if (!run && this.mouthTimer) {
      window.clearInterval(this.mouthTimer);
      this.mouthTimer = 0;
    }
  }
}
