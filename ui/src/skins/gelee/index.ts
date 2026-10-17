// Skin Gelée : bonbon de verre dépoli avec des lumières colorées qui nagent
// dedans ; la couleur des lumières dit ce que fait Kobi. Port du prototype
// docs/design/mascotte/Gelee.dc.html (CSS repris) ; Fait et Oups, absents du
// prototype, sont ajoutés dans le même style. Skin le plus coûteux (flous).

import type { MascotData, MascotState, Skin, SkinEnv } from '../../contract';
import './gelee.css';

const STATUS: Record<MascotState, string> = {
  idle: 'en veille',
  listen: 'écoute…',
  think: 'réfléchit…',
  write: 'prend des notes',
  answer: 'répond',
  suggest: 'a une idée',
  done: 'c’est fait',
  error: 'oups',
};

/** Couleurs des trois lumières, par état. */
export const COLORS: Record<MascotState, [string, string, string]> = {
  idle: ['#A5B4CF', '#C7CCE0', '#B8C2D8'],
  listen: ['#3B82F6', '#22D3EE', '#818CF8'],
  think: ['#A855F7', '#EC4899', '#6366F1'],
  write: ['#F59E0B', '#FB7185', '#FCD34D'],
  answer: ['#10B981', '#38BDF8', '#34D399'],
  suggest: ['#84CC16', '#22C55E', '#FACC15'],
  done: ['#22C55E', '#84CC16', '#10B981'],
  error: ['#EF4444', '#F97316', '#F43F5E'],
};

const MARKUP = `
<div class="wrap">
  <div class="halo"></div>
  <div class="mover">
    <div class="squircle">
      <div class="lights"><div class="b b1"></div><div class="b b2"></div><div class="b b3"></div></div>
      <div class="glass"></div>
      <div class="face"><div class="eye"></div><div class="eye"></div></div>
      <div class="bars"><div class="bar"></div><div class="bar k2"></div></div>
    </div>
  </div>
  <div class="plus"></div>
</div>
<div class="floor"></div>
<div class="status"></div>`;

export class GeleeSkin implements Skin {
  readonly id = 'gelee';
  readonly name = 'Gelée';
  readonly theme = {
    backdrop: '#E8EBF2',
    ink: '#1A1C24',
    paper: '#FFFFFF',
    accent: '#6366F1',
    muted: '#6A7083',
    danger: '#EF4444',
    font: "'Geist', system-ui, sans-serif",
    dots: Object.fromEntries(Object.entries(COLORS).map(([k, v]) => [k, v[0]])),
  };

  private root!: HTMLElement;
  private status!: HTMLElement;
  private plus!: HTMLElement;
  private state: MascotState | null = null;

  mount(host: HTMLElement, env: SkinEnv): void {
    this.root = document.createElement('div');
    this.root.className = 'skin-gelee';
    this.root.innerHTML = MARKUP;
    this.status = this.root.querySelector('.status')!;
    this.plus = this.root.querySelector('.plus')!;
    this.root.classList.toggle('reduced-motion', env.reducedMotion);
    host.append(this.root);
  }

  setState(state: MascotState, _data: Readonly<MascotData>): void {
    if (state === this.state) return;
    this.state = state;
    const keep = ['reduced-motion', 'paused'].filter((c) => this.root.classList.contains(c));
    this.root.className = `skin-gelee m-${state} ${keep.join(' ')}`.trim();
    this.root.dataset.state = state;
    const [c1, c2, c3] = COLORS[state];
    this.root.style.setProperty('--c1', c1);
    this.root.style.setProperty('--c2', c2);
    this.root.style.setProperty('--c3', c3);
    this.plus.textContent = state === 'done' ? '✓' : '+';
    this.status.textContent = STATUS[state];
  }

  setData(_data: Readonly<MascotData>): void {
    // Le bonbon ne dépend pas des données.
  }

  setPaused(paused: boolean): void {
    this.root.classList.toggle('paused', paused);
  }

  setReducedMotion(reduced: boolean): void {
    this.root.classList.toggle('reduced-motion', reduced);
  }

  destroy(): void {
    this.root.remove();
  }
}

export function createGeleeSkin(): Skin {
  return new GeleeSkin();
}
