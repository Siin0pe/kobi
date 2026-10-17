// Skin Essaim : nuée de 46 particules qui se réorganisent (anneau, ondes,
// orbites, ligne qui s'écrit, forme d'onde, « ! »). Port du prototype
// docs/design/mascotte/Essaim.dc.html : les formes (`target`, `eyes`) sont
// reprises ; le rendu passe du DOM (46 éléments recalculés toutes les 50 ms)
// à un canevas, piloté par le temps réel à 60 images/s (30 en veille), avec
// des lueurs précalculées en sprites. Fait et Oups sont ajoutés.

import type { MascotData, MascotState, Skin, SkinEnv } from '../../contract';
import './essaim.css';

const N = 46;
const TAU = Math.PI * 2;
const SIZE = 340;
/** Le prototype avançait d'un pas toutes les 50 ms : `t` compte ces pas. */
const STEP_MS = 50;

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

export const COLOR: Record<MascotState, string> = {
  idle: '#C9D1E6',
  listen: '#5EEAD4',
  think: '#C4B5FD',
  write: '#FCD34D',
  answer: '#F8FAFC',
  suggest: '#BEF264',
  done: '#86EFAC',
  error: '#FCA5A5',
};

interface Point {
  x: number;
  y: number;
  o: number;
}

/** Position cible de la particule i au pas t (formules du prototype). */
export function target(mode: MascotState, i: number, t: number): Point {
  let a: number;
  let r: number;
  let x = 0;
  let y = 0;
  let o = 1;
  switch (mode) {
    case 'idle':
      a = (i / N) * TAU + t * 0.01;
      r = 78 * (1 + 0.04 * Math.sin(t * 0.08 + i));
      x = r * Math.cos(a);
      y = r * Math.sin(a);
      break;
    case 'listen':
      a = (i / N) * TAU;
      r = 84 + 16 * Math.sin(t * 0.35 + i * 0.8);
      x = r * Math.cos(a);
      y = r * Math.sin(a);
      break;
    case 'think': {
      const k = i % 3;
      const idx = Math.floor(i / 3);
      const th = (k * Math.PI) / 3;
      a = (idx / 16) * TAU + t * 0.12 * (k === 1 ? -1 : 1);
      const x0 = 100 * Math.cos(a);
      const y0 = 34 * Math.sin(a);
      x = x0 * Math.cos(th) - y0 * Math.sin(th);
      y = x0 * Math.sin(th) + y0 * Math.cos(th);
      break;
    }
    case 'write':
      if (i < 28) {
        a = (i / 28) * TAU + t * 0.02;
        x = 62 * Math.cos(a);
        y = 62 * Math.sin(a) - 24;
      } else {
        const j = i - 28;
        x = -85 + j * 10;
        y = 92;
        o = j < (t % 46) / 2 ? 1 : 0;
      }
      break;
    case 'answer': {
      x = -120 + i * (240 / (N - 1));
      const env = Math.sin((Math.PI * i) / (N - 1));
      y = 34 + Math.sin(i * 0.5 + t * 0.45) * 30 * env * (0.6 + 0.4 * Math.abs(Math.sin(t * 0.13)));
      break;
    }
    case 'suggest': {
      const b = -Math.abs(Math.sin(t * 0.22)) * 22;
      if (i < 36) {
        a = (i / 36) * TAU + t * 0.03;
        x = 70 * Math.cos(a);
        y = 70 * Math.sin(a) + b;
      } else if (i < 44) {
        x = 125;
        y = -78 + (i - 36) * 8 + b;
      } else {
        x = 125;
        y = -2 + b;
      }
      break;
    }
    case 'done': {
      // Une coche qui scintille.
      const u = i / (N - 1);
      const [ax, ay, bx, by, cx, cy] = [-70, 10, -20, 60, 80, -60];
      const split = 0.32;
      if (u < split) {
        x = ax + ((bx - ax) * u) / split;
        y = ay + ((by - ay) * u) / split;
      } else {
        x = bx + ((cx - bx) * (u - split)) / (1 - split);
        y = by + ((cy - by) * (u - split)) / (1 - split);
      }
      y += Math.sin(t * 0.3 + i) * 2;
      o = 0.7 + 0.3 * Math.sin(t * 0.4 + i * 0.7);
      break;
    }
    case 'error': {
      // L'anneau se brise et retombe en grésillant.
      a = (i / N) * TAU + 0.6;
      r = 78;
      const fall = Math.max(0, Math.sin(a)) * 26;
      x = r * Math.cos(a) + Math.sin(t * 2.3 + i * 7) * 2.5;
      y = r * Math.sin(a) * 0.8 + fall + 18 + Math.cos(t * 1.9 + i * 5) * 2.5;
      o = i % 9 === 0 ? 0.25 : 1;
      break;
    }
  }
  return { x, y, o };
}

/** Yeux : décalage et taille, par état (prototype + Fait et Oups). */
function eyes(mode: MascotState, t: number): { dx: number; dy: number; w: number; h: number } {
  const b = mode === 'suggest' ? -Math.abs(Math.sin(t * 0.22)) * 22 : 0;
  const m: Record<MascotState, [number, number, number, number]> = {
    idle: [0, -6, 16, 4],
    listen: [0, -8, 16, 26],
    think: [12, -18, 12, 18],
    write: [-8, -14, 12, 14],
    answer: [0, -44, 18, 7],
    suggest: [0, -10 + b, 16, 26],
    done: [-10, -70, 18, 7],
    error: [0, -4, 16, 5],
  };
  const [dx, dy, w, h] = m[mode];
  return { dx, dy, w, h };
}

/** Sprite de particule avec sa lueur, dessiné une fois par couleur et par taille. */
function sprite(color: string, size: number, dpr: number): HTMLCanvasElement {
  const glow = 10;
  const px = Math.ceil((size + glow * 2) * dpr);
  const c = document.createElement('canvas');
  c.width = c.height = px;
  const g = c.getContext('2d')!;
  g.scale(dpr, dpr);
  const mid = size / 2 + glow;
  g.shadowColor = color;
  g.shadowBlur = glow;
  g.fillStyle = color;
  g.beginPath();
  g.arc(mid, mid, size / 2, 0, TAU);
  g.fill();
  return c;
}

export class EssaimSkin implements Skin {
  readonly id = 'essaim';
  readonly name = 'Essaim';
  readonly theme = {
    backdrop: '#05060A',
    ink: '#05060A',
    paper: '#E5E7EB',
    accent: '#5EEAD4',
    muted: '#6B7280',
    danger: '#FCA5A5',
    font: "'Sora', system-ui, sans-serif",
    dots: COLOR,
  };

  private root!: HTMLElement;
  private canvas!: HTMLCanvasElement;
  private ctx!: CanvasRenderingContext2D;
  private status!: HTMLElement;
  private dpr = 1;
  private sprites = new Map<string, HTMLCanvasElement>();
  private pos: Point[] = Array.from({ length: N }, () => ({ x: 0, y: 0, o: 0 }));
  private eye = { x: 0, y: 0, w: 16, h: 4 };
  private state: MascotState = 'idle';
  private t = 0;
  private last = 0;
  private lastDraw = 0;
  private frame = 0;
  private paused = false;
  private reduced = false;

  mount(host: HTMLElement, env: SkinEnv): void {
    this.reduced = env.reducedMotion;
    this.root = document.createElement('div');
    this.root.className = 'skin-essaim';
    this.canvas = document.createElement('canvas');
    this.dpr = Math.min(2, window.devicePixelRatio || 1);
    this.canvas.width = this.canvas.height = SIZE * this.dpr;
    this.ctx = this.canvas.getContext('2d')!;
    this.status = document.createElement('div');
    this.status.className = 'status';
    this.root.append(this.canvas, this.status);
    host.append(this.root);
  }

  setState(state: MascotState, _data: Readonly<MascotData>): void {
    const first = !this.root.dataset.state;
    if (state === this.state && !first) return;
    this.state = state;
    this.root.dataset.state = state;
    this.root.style.setProperty('--color', COLOR[state]);
    this.status.textContent = STATUS[state];
    if (this.reduced) this.settle();
    this.sync();
  }

  setData(_data: Readonly<MascotData>): void {
    // La nuée ne dépend pas des données.
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    this.sync();
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
    if (reduced) this.settle();
    this.sync();
  }

  destroy(): void {
    cancelAnimationFrame(this.frame);
    this.root.remove();
  }

  /** Boucle d'animation seulement fenêtre visible et animations permises. */
  private sync(): void {
    const run = !this.paused && !this.reduced;
    if (run && !this.frame) {
      this.last = performance.now();
      this.frame = requestAnimationFrame(this.tick);
    } else if (!run && this.frame) {
      cancelAnimationFrame(this.frame);
      this.frame = 0;
    }
    // Indique aux tests qu'une boucle de dessin tourne (invisible pour getAnimations()).
    this.root.dataset.animating = run ? '1' : '0';
  }

  /** Réduire les animations : la nuée se pose directement sur sa forme. */
  private settle(): void {
    for (let i = 0; i < N; i++) this.pos[i] = target(this.state, i, 0);
    const e = eyes(this.state, 0);
    this.eye = { x: e.dx, y: e.dy, w: e.w, h: e.h };
    this.draw();
  }

  private readonly tick = (now: number): void => {
    this.frame = requestAnimationFrame(this.tick);
    // En veille, la nuée tourne lentement : 30 images/s suffisent.
    if (this.state === 'idle' && now - this.lastDraw < 32) return;
    const dt = Math.min(now - this.last, 100);
    this.last = now;
    this.lastDraw = now;
    const steps = dt / STEP_MS;
    this.t += steps;
    // Même amortissement que le prototype (0,18 et 0,3 par pas de 50 ms), rapporté au temps écoulé.
    const kp = 1 - Math.pow(1 - 0.18, steps);
    const ko = 1 - Math.pow(1 - 0.3, steps);
    const ke = 1 - Math.pow(1 - 0.2, steps);
    for (let i = 0; i < N; i++) {
      const g = target(this.state, i, this.t);
      const p = this.pos[i]!;
      p.x += (g.x - p.x) * kp;
      p.y += (g.y - p.y) * kp;
      p.o += (g.o - p.o) * ko;
    }
    const e = eyes(this.state, this.t);
    this.eye.x += (e.dx - this.eye.x) * ke;
    this.eye.y += (e.dy - this.eye.y) * ke;
    this.eye.w += (e.w - this.eye.w) * kp;
    this.eye.h += (e.h - this.eye.h) * kp;
    this.draw();
  };

  private spriteFor(color: string, size: number): HTMLCanvasElement {
    const key = `${color}/${size}`;
    let s = this.sprites.get(key);
    if (!s) {
      s = sprite(color, size, this.dpr);
      this.sprites.set(key, s);
    }
    return s;
  }

  private draw(): void {
    const g = this.ctx;
    const color = COLOR[this.state];
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.clearRect(0, 0, this.canvas.width, this.canvas.height);
    g.setTransform(this.dpr, 0, 0, this.dpr, (SIZE / 2) * this.dpr, (SIZE / 2) * this.dpr);
    for (let i = 0; i < N; i++) {
      const p = this.pos[i]!;
      if (p.o < 0.02) continue;
      const size = 4 + (i % 4);
      const s = this.spriteFor(color, size);
      const half = s.width / this.dpr / 2;
      g.globalAlpha = Math.min(1, p.o);
      g.drawImage(s, p.x - half, p.y - half, s.width / this.dpr, s.height / this.dpr);
    }
    g.globalAlpha = 1;
    g.fillStyle = '#FFFFFF';
    g.shadowColor = 'rgba(255,255,255,.9)';
    g.shadowBlur = 14;
    const { x, y, w, h } = this.eye;
    for (const ox of [-22, 22]) {
      g.beginPath();
      g.roundRect(x + ox - w / 2, y - h / 2, w, h, Math.min(w, h) / 2);
      g.fill();
    }
    g.shadowBlur = 0;
  }
}

export function createEssaimSkin(): Skin {
  return new EssaimSkin();
}
