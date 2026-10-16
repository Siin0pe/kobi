// Skin Kobi, style Glyphe : un visage fait de lettres. Port du prototype
// docs/design/mascotte/Glyphe.dc.html ; le CSS et les keyframes sont repris,
// la logique (horloge de 60 ms qui redessinait tout) est réécrite : seules
// les animations de l'état courant tournent, la plupart dans le moteur CSS.

import type { MascotData, MascotState, Skin, SkinEnv } from '../../contract';
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

/** Bouche qui parle (Répond), une image toutes les 120 ms comme le prototype. */
const MOUTH = ['o', 'O', 'o', '-', 'O', 'o'];
const MOUTH_MS = 120;

function el(tag: string, className?: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export class KobiSkin implements Skin {
  readonly id = 'kobi';
  readonly name = 'Kobi';
  readonly theme = {
    backdrop: '#D7FF3A',
    ink: '#0B0B0B',
    paper: '#F7FFE0',
    accent: '#D7FF3A',
    muted: '#3A4200',
    danger: '#FF4D2E',
    font: "'Recursive', system-ui, sans-serif",
  };

  private root!: HTMLElement;
  private pill!: HTMLElement;
  private eyes!: HTMLElement[];
  private mouth!: HTMLElement;
  private word!: HTMLElement;
  private orbit = new Orbit();
  private inertia!: Inertia;
  private state: MascotState | null = null;
  private reduced = false;
  private paused = false;
  private mouthTimer = 0;
  private mouthStep = 0;

  mount(host: HTMLElement, env: SkinEnv): void {
    this.reduced = env.reducedMotion;
    this.root = el('div', 'skin-kobi');
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
    host.append(this.root);
  }

  setState(state: MascotState, _data: Readonly<MascotData>): void {
    if (state === this.state) return;
    const previous = this.state;
    this.state = state;
    const look = LOOKS[state];

    this.inertia.carry(() => {
      this.root.className = `skin-kobi m-${state}`;
      this.root.classList.toggle('reduced-motion', this.reduced);
      this.root.classList.toggle('paused', this.paused);
    }, previous !== null && !this.reduced);

    this.root.dataset.state = state;
    for (const eye of this.eyes) eye.textContent = look.eye;
    this.setWord(look.word);
    this.mouthStep = 0;
    this.mouth.textContent = state === 'error' ? '~' : state === 'answer' ? MOUTH[0]! : '';

    if (state === 'think') this.orbit.start(this.reduced);
    else this.orbit.stop();
    if (this.paused) this.orbit.pause(true);
    this.syncMouth();
  }

  setData(_data: Readonly<MascotData>): void {
    // La mascotte ne dépend pas des données : les cartes communes les affichent.
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    this.root.classList.toggle('paused', paused);
    this.orbit.pause(paused || this.reduced);
    this.inertia.pause(paused);
    this.syncMouth();
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
    this.root.classList.toggle('reduced-motion', reduced);
    if (reduced) this.inertia.cancel();
    this.orbit.pause(reduced || this.paused);
    this.syncMouth();
  }

  destroy(): void {
    window.clearInterval(this.mouthTimer);
    this.orbit.stop();
    this.inertia.cancel();
    this.root.remove();
  }

  private setWord(text: string): void {
    const letters = [...text].map((c, i) => {
      const span = el('span', 'ch', c);
      span.style.animationDelay = `${(i * 0.12).toFixed(2)}s`;
      return span;
    });
    this.word.replaceChildren(...letters);
  }

  /** La bouche ne s'anime qu'en Répond, fenêtre visible, animations permises. */
  private syncMouth(): void {
    const run = this.state === 'answer' && !this.paused && !this.reduced;
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

export function createKobiSkin(): Skin {
  return new KobiSkin();
}
