// Skin Matrice : petit écran à points LED orange dans un boîtier noir, chaque
// expression dessinée pixel par pixel. Port du prototype
// docs/design/mascotte/Matrice.dc.html : mêmes images (fonction `frame`
// reprise), même cadence de 10 images/s. Les expressions fixes (Veille, Fait)
// ne font tourner aucune minuterie : seule la respiration de la veille, en
// CSS, reste animée. Fait et Oups, absents du prototype, sont ajoutés.

import type { MascotData, MascotState, Skin, SkinEnv } from '../../contract';
import './matrice.css';

export const W = 15;
export const H = 9;
const STEP_MS = 100;

const STATUS: Record<MascotState, string> = {
  idle: 'en veille',
  listen: 'écoute',
  think: 'réfléchit',
  write: 'prend des notes',
  answer: 'répond',
  suggest: 'a une idée',
  done: 'c’est fait',
  error: 'oups',
};

/** États dont l'image change avec le temps (les autres sont fixes). */
const ANIMATED: ReadonlySet<MascotState> = new Set(['listen', 'think', 'write', 'answer', 'suggest', 'error']);

const PERIM: Array<[number, number]> = (() => {
  const p: Array<[number, number]> = [];
  for (let c = 0; c < W; c++) p.push([0, c]);
  for (let r = 1; r < H; r++) p.push([r, W - 1]);
  for (let c = W - 2; c >= 0; c--) p.push([H - 1, c]);
  for (let r = H - 2; r > 0; r--) p.push([r, 0]);
  return p;
})();

function rect(on: Set<number>, r0: number, r1: number, c0: number, c1: number): void {
  for (let r = r0; r <= r1; r++) for (let c = c0; c <= c1; c++) on.add(r * W + c);
}

/** Points allumés de l'image `t` (pas de 100 ms) d'un état. */
export function frame(mode: MascotState, t: number): Set<number> {
  const on = new Set<number>();
  switch (mode) {
    case 'idle':
      rect(on, 5, 5, 3, 5);
      rect(on, 5, 5, 9, 11);
      break;
    case 'listen':
      rect(on, 2, 5, 3, 5);
      rect(on, 2, 5, 9, 11);
      [0, 1, 13, 14].forEach((col, k) => {
        const h = Math.round(1 + 2.6 * Math.abs(Math.sin(t * 0.55 + k * 1.7)));
        for (let r = 4 - h; r <= 4 + h; r++) if (r >= 0 && r < H) on.add(r * W + col);
      });
      break;
    case 'think': {
      rect(on, 2, 3, 5, 6);
      rect(on, 2, 3, 10, 11);
      const L = PERIM.length;
      const head = t % L;
      for (let i = 0; i < 4; i++) {
        const [r, c] = PERIM[(head - i + L) % L]!;
        on.add(r * W + c);
      }
      break;
    }
    case 'write': {
      rect(on, 3, 4, 3, 4);
      rect(on, 3, 4, 9, 10);
      const n = t % 18;
      for (let c = 1; c <= Math.min(13, n); c++) if (c !== 4 && c !== 9) on.add(7 * W + c);
      if (t % 4 < 2) on.add(7 * W + Math.min(13, n + 1));
      break;
    }
    case 'answer':
      for (const [r, c] of [[3, 3], [2, 4], [3, 5], [3, 9], [2, 10], [3, 11]] as const) on.add(r * W + c);
      for (let c = 5; c <= 9; c++) {
        const h = Math.floor(1.9 * Math.abs(Math.sin(t * 0.9 + c * 1.3)));
        for (let r = 6 - h; r <= 6 + h; r++) on.add(r * W + c);
      }
      break;
    case 'suggest':
      rect(on, 2, 5, 2, 4);
      rect(on, 2, 5, 8, 10);
      if (t % 6 < 4) {
        rect(on, 1, 5, 13, 13);
        on.add(7 * W + 13);
      }
      break;
    case 'done':
      // Yeux rieurs et grand sourire.
      for (const [r, c] of [[3, 3], [2, 4], [3, 5], [3, 9], [2, 10], [3, 11]] as const) on.add(r * W + c);
      for (const [r, c] of [[5, 3], [6, 4], [7, 5], [7, 6], [7, 7], [7, 8], [7, 9], [6, 10], [5, 11]] as const) on.add(r * W + c);
      break;
    case 'error': {
      // Yeux en croix ; la bouche en zigzag grésille.
      for (const c0 of [3, 9]) for (const [dr, dc] of [[0, 0], [0, 2], [1, 1], [2, 0], [2, 2]] as const) on.add((2 + dr) * W + c0 + dc);
      const shift = t % 4 < 2 ? 0 : 1;
      for (let c = 4; c <= 10; c++) on.add((6 + ((c + shift) % 2)) * W + c);
      break;
    }
  }
  return on;
}

export class MatriceSkin implements Skin {
  readonly id = 'matrice';
  readonly name = 'Matrice';
  readonly theme = {
    backdrop: '#F2F2EF',
    ink: '#111111',
    paper: '#FFFFFF',
    accent: '#FF4D1F',
    muted: '#6B6B66',
    danger: '#FF4D1F',
    font: "'Space Mono', ui-monospace, monospace",
    dots: { idle: '#1D1D1D', listen: '#FF4D1F', think: '#FF4D1F', suggest: '#FF4D1F' },
  };

  private root!: HTMLElement;
  private dots: HTMLElement[] = [];
  private status!: HTMLElement;
  private state: MascotState = 'idle';
  private t = 0;
  private timer = 0;
  private lit = new Set<number>();
  private paused = false;
  private reduced = false;

  mount(host: HTMLElement, env: SkinEnv): void {
    this.reduced = env.reducedMotion;
    this.root = document.createElement('div');
    this.root.className = 'skin-matrice';
    const dev = document.createElement('div');
    dev.className = 'dev';
    const grid = document.createElement('div');
    grid.className = 'grid';
    for (let i = 0; i < W * H; i++) {
      const d = document.createElement('span');
      d.className = 'led';
      grid.append(d);
      this.dots.push(d);
    }
    dev.append(grid);
    const label = document.createElement('div');
    label.className = 'label';
    this.status = document.createElement('span');
    label.append(document.createElement('i'), this.status);
    this.root.append(dev, label);
    host.append(this.root);
  }

  setState(state: MascotState, _data: Readonly<MascotData>): void {
    const first = !this.root.dataset.state;
    if (state === this.state && !first) return;
    this.state = state;
    this.t = 0;
    this.root.className = `skin-matrice m-${state}`;
    this.root.classList.toggle('reduced-motion', this.reduced);
    this.root.classList.toggle('paused', this.paused);
    this.root.dataset.state = state;
    this.status.textContent = STATUS[state];
    this.paint();
    this.sync();
  }

  setData(_data: Readonly<MascotData>): void {
    // L'écran ne dépend pas des données.
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    this.root.classList.toggle('paused', paused);
    this.sync();
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
    this.root.classList.toggle('reduced-motion', reduced);
    this.sync();
  }

  destroy(): void {
    window.clearInterval(this.timer);
    this.root.remove();
  }

  /** Minuterie de 100 ms seulement pour les images animées, fenêtre visible. */
  private sync(): void {
    const run = ANIMATED.has(this.state) && !this.paused && !this.reduced;
    if (run && !this.timer) {
      this.timer = window.setInterval(() => {
        this.t += 1;
        this.paint();
      }, STEP_MS);
    } else if (!run && this.timer) {
      window.clearInterval(this.timer);
      this.timer = 0;
    }
  }

  /** N'allume ou n'éteint que les points qui changent. */
  private paint(): void {
    const next = frame(this.state, this.t);
    for (const i of this.lit) if (!next.has(i)) this.dots[i]!.classList.remove('on');
    for (const i of next) if (!this.lit.has(i)) this.dots[i]!.classList.add('on');
    this.lit = next;
  }
}

export function createMatriceSkin(): Skin {
  return new MatriceSkin();
}
