// Symboles `{ } < / > #` en orbite 3D autour de la pilule (Réfléchit).
//
// Le prototype recalculait la position de chaque symbole toutes les 60 ms en
// JavaScript. Ici, une révolution est précalculée en images clés et jouée par
// le moteur d'animation (Web Animations) : aucune boucle JavaScript, et 60
// images/s au lieu de 16.

const SYMBOLS = ['{', '}', '<', '/', '>', '#'];
/** Vitesse du prototype : 0,07 rad par pas de 60 ms. */
const PERIOD_MS = ((2 * Math.PI) / 0.07) * 60;
const STEPS = 48;
const RX = 210;
const RY = 74;

function keyframes(): Keyframe[] {
  const frames: Keyframe[] = [];
  for (let s = 0; s <= STEPS; s++) {
    const a = (s / STEPS) * Math.PI * 2;
    const depth = (Math.sin(a) + 1) / 2;
    frames.push({
      offset: s / STEPS,
      transform: `translate(${(Math.cos(a) * RX).toFixed(1)}px,${(Math.sin(a) * RY).toFixed(1)}px) scale(${(0.7 + 0.5 * depth).toFixed(3)})`,
      opacity: (0.3 + 0.7 * depth).toFixed(3),
      zIndex: depth > 0.5 ? '3' : '1',
      fontVariationSettings: `"wght" ${Math.round(300 + 700 * depth)}, "CASL" 1`,
    });
  }
  return frames;
}

export class Orbit {
  readonly root: HTMLElement;
  private readonly spans: HTMLElement[];
  private anims: Animation[] = [];

  constructor() {
    this.root = document.createElement('div');
    this.root.className = 'orbit';
    this.spans = SYMBOLS.map((ch) => {
      const span = document.createElement('span');
      span.textContent = ch;
      this.root.append(span);
      return span;
    });
  }

  /** Lance l'orbite ; avec `still`, pose les symboles sans mouvement. */
  start(still: boolean): void {
    this.stop();
    const frames = keyframes();
    this.anims = this.spans.map((span, i) => {
      const anim = span.animate(frames, {
        duration: PERIOD_MS,
        iterations: Infinity,
        delay: -(i / SYMBOLS.length) * PERIOD_MS,
      });
      if (still) anim.pause();
      return anim;
    });
  }

  pause(paused: boolean): void {
    for (const a of this.anims) {
      if (paused) a.pause();
      else a.play();
    }
  }

  stop(): void {
    this.anims.forEach((a) => a.cancel());
    this.anims = [];
  }
}
