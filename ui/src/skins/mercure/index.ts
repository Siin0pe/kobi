// Skin Mercure : goutte de chrome liquide qui coule, s'étire et se dédouble,
// un seul accent bleu électrique. Port du prototype
// docs/design/mascotte/Mercure.dc.html (CSS repris) ; Fait et Oups, absents
// du prototype, sont ajoutés dans le même style.

import type { MascotData, MascotState, Skin, SkinEnv } from '../../contract';
import './mercure.css';

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

const MARKUP = `
<div class="body">
  <div class="rings"><div class="ring"></div><div class="ring r2"></div></div>
  <div class="sat"><i></i><i></i><i></i></div>
  <div class="float">
    <div class="drop">
      <div class="chrome"></div>
      <div class="tint"></div>
      <div class="hl"></div>
      <div class="face"><div class="eye"></div><div class="eye"></div></div>
    </div>
  </div>
  <div class="drip"></div>
  <div class="bub"></div>
</div>
<div class="shadow"></div>
<div class="status"></div>`;

export class MercureSkin implements Skin {
  readonly id = 'mercure';
  readonly name = 'Mercure';
  readonly theme = {
    backdrop: 'radial-gradient(circle at 72% 42%, #1A1B20 0%, #08080A 60%)',
    ink: '#08080A',
    paper: '#E9EBEF',
    accent: '#4D7CFF',
    muted: '#5A5F6A',
    danger: '#FF5A5A',
    font: "'JetBrains Mono', ui-monospace, monospace",
    dots: { idle: '#9CA1AB', listen: '#4D7CFF', suggest: '#4D7CFF' },
  };

  private root!: HTMLElement;
  private bub!: HTMLElement;
  private status!: HTMLElement;
  private state: MascotState | null = null;

  mount(host: HTMLElement, env: SkinEnv): void {
    this.root = document.createElement('div');
    this.root.className = 'skin-mercure';
    this.root.innerHTML = MARKUP;
    this.bub = this.root.querySelector('.bub')!;
    this.status = this.root.querySelector('.status')!;
    this.root.classList.toggle('reduced-motion', env.reducedMotion);
    host.append(this.root);
  }

  setState(state: MascotState, _data: Readonly<MascotData>): void {
    if (state === this.state) return;
    this.state = state;
    const reduced = this.root.classList.contains('reduced-motion');
    const paused = this.root.classList.contains('paused');
    this.root.className = `skin-mercure m-${state}`;
    this.root.classList.toggle('reduced-motion', reduced);
    this.root.classList.toggle('paused', paused);
    this.root.dataset.state = state;
    this.bub.textContent = state === 'done' ? '✓' : '+';
    this.status.textContent = STATUS[state];
  }

  setData(_data: Readonly<MascotData>): void {
    // La goutte ne dépend pas des données.
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

export function createMercureSkin(): Skin {
  return new MercureSkin();
}
