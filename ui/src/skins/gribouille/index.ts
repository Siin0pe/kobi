// Skin Gribouille : petit carnet vivant dessiné à la craie, au trait qui
// tremble (filtre de turbulence SVG), animé en stop motion. Port du prototype
// docs/design/mascotte/Gribouille.dc.html (SVG et CSS repris) ; les états
// Fait et Oups, absents du prototype, sont dessinés dans le même style.

import type { MascotData, MascotState, Skin, SkinEnv } from '../../contract';
import './gribouille.css';

/** Couleur du voyant de l'antenne et mot sous la mascotte, par état. */
const LOOKS: Record<MascotState, { bulb: string; status: string }> = {
  idle: { bulb: '#4A4A55', status: 'zzz…' },
  listen: { bulb: '#FF5A36', status: 'j’écoute !' },
  think: { bulb: '#FFD23F', status: 'hmm…' },
  write: { bulb: '#EDEBE4', status: 'je note…' },
  answer: { bulb: '#FF5A36', status: 'alors…' },
  suggest: { bulb: '#FFD23F', status: 'j’ai une idée !' },
  done: { bulb: '#FFD23F', status: 'c’est fait !' },
  error: { bulb: '#FF5A36', status: 'oups…' },
};

const SVG = `
<svg class="gb" width="240" height="240" viewBox="0 0 240 240" aria-hidden="true">
  <defs>
    <filter id="gb-boil" x="-10%" y="-10%" width="120%" height="120%">
      <feTurbulence type="fractalNoise" baseFrequency="0.035" numOctaves="2" seed="1">
        <animate attributeName="seed" values="1;5;9;13" dur="0.48s" calcMode="discrete" repeatCount="indefinite"></animate>
      </feTurbulence>
      <feDisplacementMap in="SourceGraphic" scale="3.4"></feDisplacementMap>
    </filter>
  </defs>
  <g filter="url(#gb-boil)" stroke="#EDEBE4" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round" fill="none">
    <g class="hop">
      <g class="ant">
        <path d="M120 47 C120 34 124 28 130 22"></path>
        <circle class="bulb" cx="131" cy="18" r="7"></circle>
      </g>
      <path d="M62 74 C60 52 82 46 120 46 C158 46 180 52 178 76 L181 150 C182 176 162 184 120 184 C78 184 58 176 59 150 Z" fill="#1E1E24"></path>
      <path d="M98 184 l-2 14 l-8 2 M142 184 l2 14 l8 2"></path>
      <path d="M84 124 l9 -1 M147 123 l9 1" stroke="#FF5A36" stroke-width="3"></path>
      <path class="brow bl" d="M90 88 q9 -6 18 0"></path>
      <path class="brow br" d="M132 88 q9 -6 18 0"></path>
      <circle class="eye" cx="100" cy="105" r="6.5" fill="#EDEBE4" stroke="none"></circle>
      <circle class="eye" cx="140" cy="105" r="6.5" fill="#EDEBE4" stroke="none"></circle>
      <path class="happy" d="M93 108 q7 -10 14 0 M133 108 q7 -10 14 0"></path>
      <path class="xeyes" d="M94 99 l12 12 M106 99 l-12 12 M134 99 l12 12 M146 99 l-12 12"></path>
      <path class="mo mo-smile" d="M111 128 q9 8 18 0"></path>
      <circle class="mo mo-o" cx="124" cy="131" r="4"></circle>
      <ellipse class="mo mo-open" cx="120" cy="131" rx="8" ry="7" fill="#0E0E11"></ellipse>
      <path class="mo mo-wavy" d="M108 132 q4 -5 8 0 q4 5 8 0 q4 -5 8 0"></path>

      <g class="arms a-idle"><path d="M60 128 q-12 10 -10 24"></path><path d="M180 128 q12 10 10 24"></path></g>
      <g class="arms a-listen"><path d="M60 128 q-12 10 -10 24"></path><path d="M180 122 q20 -4 22 -28"></path><path d="M196 92 q12 -2 12 10"></path></g>
      <g class="arms a-think"><path d="M60 128 q-12 10 -10 24"></path><path d="M180 130 q6 24 -36 20"></path><circle cx="141" cy="149" r="5" fill="#1E1E24"></circle></g>
      <g class="arms a-write">
        <path d="M62 136 q12 18 36 18"></path><path d="M178 136 q-10 14 -30 16"></path>
        <rect x="92" y="148" width="58" height="34" rx="3" fill="#EDEBE4"></rect>
        <path class="line" d="M100 158 h40" stroke="#17171B" stroke-width="2.4"></path>
        <path class="line l2" d="M100 166 h32" stroke="#17171B" stroke-width="2.4"></path>
        <path class="line l3" d="M100 174 h22" stroke="#17171B" stroke-width="2.4"></path>
        <g class="pen"><path d="M146 160 l16 -26" stroke-width="4.2"></path><path d="M160 137 l5 -8" stroke="#FFD23F" stroke-width="4.2"></path></g>
      </g>
      <g class="arms a-answer"><path d="M60 128 q-12 10 -10 24"></path><path d="M180 124 q18 4 24 -12"></path><circle cx="206" cy="108" r="4.5" fill="#1E1E24"></circle></g>
      <g class="arms a-suggest"><path d="M60 128 q-12 10 -10 24"></path><path d="M180 118 q18 -10 18 -42"></path><circle cx="198" cy="70" r="6" fill="#1E1E24"></circle></g>
      <g class="arms a-done"><path d="M60 122 q-18 -10 -18 -40"></path><circle cx="42" cy="76" r="6" fill="#1E1E24"></circle><path d="M180 122 q18 -10 18 -40"></path><circle cx="198" cy="76" r="6" fill="#1E1E24"></circle></g>
      <g class="arms a-error"><path d="M60 132 q-6 14 -2 30"></path><path d="M180 132 q6 14 2 30"></path></g>
    </g>

    <g class="x x-zz" stroke-width="2.6">
      <path class="zz1" d="M168 34 h10 l-10 10 h10"></path>
      <path class="zz2" d="M186 16 h8 l-8 8 h8"></path>
    </g>
    <g class="x x-waves">
      <path class="wave" d="M214 80 q7 12 0 26"></path>
      <path class="wave w2" d="M224 72 q13 20 0 42"></path>
    </g>
    <g class="x x-spiral">
      <path class="spiral" d="M76 30 c0 -8 12 -8 12 0 c0 12 -22 12 -22 0 c0 -16 32 -16 32 0" stroke="#FFD23F"></path>
    </g>
    <g class="x x-bang" stroke="#FFD23F">
      <g class="bang"><path d="M218 30 v20"></path><circle cx="218" cy="60" r="2.4" fill="#FFD23F"></circle></g>
      <path d="M204 22 l-5 -7 M232 26 l7 -5 M232 46 l8 2"></path>
    </g>
    <g class="x x-check" stroke="#FFD23F" stroke-width="4">
      <path class="check" d="M200 40 l9 10 l20 -26"></path>
      <path d="M190 22 l-5 -7 M236 44 l8 2" stroke-width="3"></path>
    </g>
    <g class="x x-sweat" stroke="#8FC7FF" stroke-width="2.6">
      <path class="drop" d="M188 60 q-6 9 0 12 q6 -3 0 -12 Z" fill="#8FC7FF"></path>
    </g>
  </g>
</svg>`;

export class GribouilleSkin implements Skin {
  readonly id = 'gribouille';
  readonly name = 'Gribouille';
  readonly theme = {
    backdrop: 'radial-gradient(#26262D 1.2px, transparent 1.2px) 0 0 / 22px 22px, #131317',
    ink: '#17171B',
    paper: '#EDEBE4',
    accent: '#FF5A36',
    muted: '#55524A',
    danger: '#FF5A36',
    font: "'Atkinson Hyperlegible', system-ui, sans-serif",
    dots: { idle: '#4A4A55', listen: '#FF5A36', think: '#FFD23F', suggest: '#FFD23F' },
  };

  private root!: HTMLElement;
  private svg!: SVGSVGElement;
  private bulb!: SVGCircleElement;
  private status!: HTMLElement;
  private state: MascotState | null = null;
  private paused = false;
  private reduced = false;

  mount(host: HTMLElement, env: SkinEnv): void {
    this.reduced = env.reducedMotion;
    this.root = document.createElement('div');
    this.root.className = 'skin-gribouille';
    this.root.innerHTML = `${SVG}<div class="status"></div>`;
    this.svg = this.root.querySelector('svg')!;
    this.bulb = this.root.querySelector('.bulb')!;
    this.status = this.root.querySelector('.status')!;
    host.append(this.root);
    this.syncMotion();
  }

  setState(state: MascotState, _data: Readonly<MascotData>): void {
    if (state === this.state) return;
    this.state = state;
    const look = LOOKS[state];
    this.root.className = `skin-gribouille m-${state}`;
    this.root.dataset.state = state;
    this.bulb.setAttribute('fill', look.bulb);
    this.status.textContent = look.status;
    this.syncMotion();
  }

  setData(_data: Readonly<MascotData>): void {
    // Le dessin ne dépend pas des données.
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    this.syncMotion();
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
    this.syncMotion();
  }

  destroy(): void {
    this.root.remove();
  }

  /** Les classes CSS et l'animation SMIL du trait (que le CSS ne pause pas). */
  private syncMotion(): void {
    this.root.classList.toggle('reduced-motion', this.reduced);
    this.root.classList.toggle('paused', this.paused);
    if (this.paused || this.reduced) this.svg.pauseAnimations();
    else this.svg.unpauseAnimations();
  }
}

export function createGribouilleSkin(): Skin {
  return new GribouilleSkin();
}
