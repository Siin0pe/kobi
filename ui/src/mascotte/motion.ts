// Mouvement sans saut. Deux outils :
//
// - `Inertia` : les animations CSS de la pilule repartent de zéro quand la
//   classe d'état change ; la pilule sauterait de sa pose courante (penchée,
//   en plein rebond, affaissée…) à la première image de la nouvelle animation.
//   On mesure la pose avant et après le changement, et une enveloppe parente
//   rattrape l'écart en douceur.
// - `Gestures` : gestes ponctuels (anticipation, saut, rebond, sursaut…)
//   joués sur leur propre calque. Un geste qui en interrompt un autre part de
//   la pose courante : jamais de saut non plus.

const EASING = 'cubic-bezier(.2,.7,.3,1)';

export function matrixOf(node: Element): DOMMatrix {
  const t = getComputedStyle(node).transform;
  return t && t !== 'none' ? new DOMMatrix(t) : new DOMMatrix();
}

export class Inertia {
  private anim: Animation | null = null;

  constructor(
    private readonly wrapper: HTMLElement,
    private readonly moving: HTMLElement,
    private readonly duration = 280,
  ) {}

  /** Applique `change` (changement de classe) en gardant la pose continue. */
  carry(change: () => void, enabled: boolean): void {
    if (!enabled) {
      this.cancel();
      change();
      return;
    }
    const before = matrixOf(this.wrapper).multiply(matrixOf(this.moving));
    this.cancel();
    change();
    const start = matrixOf(this.moving);
    const offset = before.multiply(start.inverse());
    if (offset.isIdentity) return;
    this.anim = this.wrapper.animate([{ transform: offset.toString() }, { transform: 'none' }], {
      duration: this.duration,
      easing: EASING,
    });
    this.anim.onfinish = () => {
      this.anim = null;
    };
  }

  pause(paused: boolean): void {
    if (!this.anim) return;
    if (paused) this.anim.pause();
    else this.anim.play();
  }

  cancel(): void {
    this.anim?.cancel();
    this.anim = null;
  }
}

/** Une pose du geste : instant (0 à 1) et transformation. */
type Pose = [offset: number, transform: string];

interface GestureDef {
  duration: number;
  poses: Pose[];
  /** Instant (ms) où le corps « décolle » : fin de l'anticipation. */
  launch: number;
}

const IN = 'cubic-bezier(.45,0,.75,.4)';
const OUT = 'cubic-bezier(.2,.75,.35,1)';
const IN_OUT = 'cubic-bezier(.45,0,.35,1)';

function def(duration: number, launch: number, poses: Pose[]): GestureDef {
  return { duration, launch, poses };
}

/**
 * Gestes de Kobi, origine en bas au centre de la pilule. Chaque geste part de
 * la pose neutre et y revient ; la première étape est l'anticipation (on
 * s'écrase avant de sauter, on recule avant de foncer).
 */
export const GESTURES = {
  /** Retour en veille : soupir, on se tasse lentement. */
  settle: def(1100, 260, [
    [0, 'none'],
    [0.24, 'translateY(-7px) scale(.97,1.05)'],
    [0.62, 'translateY(5px) scale(1.07,.92)'],
    [0.82, 'translateY(-1px) scale(.99,1.01)'],
    [1, 'none'],
  ]),
  /** Écoute : sursaut d'attention, on se dresse vers la voix. */
  startle: def(620, 90, [
    [0, 'none'],
    [0.14, 'translateY(4px) scale(1.07,.9)'],
    [0.38, 'translateY(-20px) scale(.92,1.1) rotate(-2deg)'],
    [0.62, 'translateY(0) scale(1.05,.95)'],
    [0.8, 'translateY(-4px) scale(.99,1.01)'],
    [1, 'none'],
  ]),
  /** Réfléchit : on penche la tête, on la tourne de l'autre côté. */
  ponder: def(900, 150, [
    [0, 'none'],
    [0.17, 'translateY(3px) scale(1.04,.95) rotate(3deg)'],
    [0.5, 'translateY(-8px) rotate(-7deg)'],
    [0.78, 'translateY(-2px) rotate(2deg)'],
    [1, 'none'],
  ]),
  /** Note : on se penche vers la fiche (à gauche). */
  lean: def(760, 130, [
    [0, 'none'],
    [0.17, 'translateX(8px) scale(1.04,.95)'],
    [0.48, 'translateX(-18px) translateY(-6px) rotate(-6deg)'],
    [0.76, 'translateX(-4px) rotate(-1deg)'],
    [1, 'none'],
  ]),
  /** Répond : on prend sa respiration, on se lance. */
  speak: def(700, 120, [
    [0, 'none'],
    [0.17, 'translateY(3px) scale(1.06,.92)'],
    [0.42, 'translateY(-12px) scale(.95,1.07) rotate(-3deg)'],
    [0.7, 'translateY(1px) scale(1.03,.97)'],
    [1, 'none'],
  ]),
  /** Propose : grand saut avec vrille, réception écrasée. */
  pop: def(900, 140, [
    [0, 'none'],
    [0.15, 'translateY(6px) scale(1.12,.82)'],
    [0.4, 'translateY(-42px) scale(.9,1.12) rotate(-10deg)'],
    [0.6, 'translateY(0) scale(1.12,.88) rotate(2deg)'],
    [0.76, 'translateY(-10px) scale(.97,1.03) rotate(-2deg)'],
    [0.9, 'translateY(0) scale(1.02,.98)'],
    [1, 'none'],
  ]),
  /** Fait : double saut de joie. */
  cheer: def(1100, 120, [
    [0, 'none'],
    [0.11, 'translateY(5px) scale(1.1,.86)'],
    [0.3, 'translateY(-34px) scale(.92,1.1) rotate(-6deg)'],
    [0.46, 'translateY(0) scale(1.1,.88)'],
    [0.62, 'translateY(-20px) scale(.95,1.06) rotate(6deg)'],
    [0.78, 'translateY(0) scale(1.05,.95)'],
    [0.9, 'translateY(-3px)'],
    [1, 'none'],
  ]),
  /** Oups : on encaisse, recul puis secousse. */
  recoil: def(760, 90, [
    [0, 'none'],
    [0.12, 'translateX(10px) scale(.94,1.06)'],
    [0.32, 'translateX(-22px) rotate(-7deg) scale(1.06,.94)'],
    [0.52, 'translateX(10px) rotate(4deg)'],
    [0.72, 'translateX(-4px) rotate(-1deg)'],
    [1, 'none'],
  ]),
  /** Petit hochement (phrase finie, note prise). */
  nod: def(420, 0, [
    [0, 'none'],
    [0.35, 'translateY(6px) rotate(2deg) scale(1.02,.97)'],
    [0.7, 'translateY(-2px)'],
    [1, 'none'],
  ]),
  /** Coup d'épaule vers la carte d'action (Propose). */
  nudge: def(560, 0, [
    [0, 'none'],
    [0.3, 'translateX(-14px) rotate(-6deg)'],
    [0.55, 'translateX(4px) rotate(2deg)'],
    [1, 'none'],
  ]),
  /** Sursaut léger (compte à rebours de l'erreur). */
  flinch: def(300, 0, [
    [0, 'none'],
    [0.3, 'translateX(-5px) scale(.98,1.02)'],
    [0.65, 'translateX(3px)'],
    [1, 'none'],
  ]),
  /** Trouvé ! (une source répond). */
  aha: def(480, 0, [
    [0, 'none'],
    [0.25, 'translateY(-14px) scale(.95,1.06)'],
    [0.55, 'translateY(0) scale(1.05,.95)'],
    [1, 'none'],
  ]),
  /** Survol ou clic : on se fait écraser comme un bonbon. */
  boop: def(520, 0, [
    [0, 'none'],
    [0.2, 'scale(1.1,.86)'],
    [0.45, 'scale(.94,1.08) translateY(-6px)'],
    [0.7, 'scale(1.03,.97)'],
    [1, 'none'],
  ]),
  /** Veille : la tête tombe doucement… puis se redresse d'un coup. */
  doze: def(2200, 0, [
    [0, 'none'],
    [0.6, 'translateY(9px) rotate(4deg) scale(1.03,.96)'],
    [0.7, 'translateY(-10px) rotate(-2deg) scale(.96,1.05)'],
    [0.84, 'translateY(2px)'],
    [1, 'none'],
  ]),
  /** Veille : bâillement, on s'étire vers le haut. */
  stretch: def(1700, 0, [
    [0, 'none'],
    [0.35, 'translateY(-10px) scale(.94,1.1)'],
    [0.6, 'translateY(-12px) scale(.93,1.12) rotate(-2deg)'],
    [0.85, 'translateY(2px) scale(1.04,.96)'],
    [1, 'none'],
  ]),
  /** Veille : frisson. */
  shiver: def(560, 0, [
    [0, 'none'],
    [0.15, 'translateX(-3px)'],
    [0.3, 'translateX(3px)'],
    [0.45, 'translateX(-3px)'],
    [0.6, 'translateX(2px)'],
    [0.75, 'translateX(-1px)'],
    [1, 'none'],
  ]),
  /** Joie discrète (Fait) : on se dandine. */
  wiggle: def(760, 0, [
    [0, 'none'],
    [0.25, 'rotate(-5deg)'],
    [0.5, 'rotate(5deg)'],
    [0.75, 'rotate(-2deg)'],
    [1, 'none'],
  ]),
  /** Écrire à Kobi (KOB-147) : on hésite, Kobi tapote d'impatience. */
  tap: def(720, 0, [
    [0, 'none'],
    [0.15, 'translateY(-6px) rotate(-2deg)'],
    [0.3, 'translateY(0) scale(1.03,.97)'],
    [0.5, 'translateY(-6px) rotate(2deg)'],
    [0.65, 'translateY(0) scale(1.03,.97)'],
    [1, 'none'],
  ]),
  /** Écrire à Kobi : le texte envoyé est avalé (la gorge descend, on se redresse). */
  gulp: def(560, 0, [
    [0, 'none'],
    [0.25, 'translateY(8px) scale(1.1,.86)'],
    [0.55, 'translateY(-10px) scale(.94,1.08)'],
    [0.8, 'translateY(1px) scale(1.02,.98)'],
    [1, 'none'],
  ]),
} satisfies Record<string, GestureDef>;

export type Gesture = keyof typeof GESTURES;

/** Easing de chaque étape : on accélère vers la pose extrême, on freine en revenant. */
function easingFor(i: number, poses: Pose[]): string {
  if (i === 0) return IN;
  return i === poses.length - 2 ? OUT : IN_OUT;
}

/** Joue un geste à la fois sur un calque ; le suivant part de la pose courante. */
export class Gestures {
  private anim: Animation | null = null;
  current: Gesture | null = null;

  constructor(private readonly layer: HTMLElement) {}

  play(name: Gesture, speed = 1): void {
    const g = GESTURES[name];
    const from = this.anim ? matrixOf(this.layer).toString() : 'none';
    this.anim?.cancel();
    const frames: Keyframe[] = g.poses.map(([offset, transform], i) => ({
      offset,
      transform: i === 0 ? from : transform,
      easing: easingFor(i, g.poses),
    }));
    this.current = name;
    const anim = this.layer.animate(frames, { duration: g.duration / speed });
    anim.onfinish = () => {
      if (this.anim !== anim) return;
      this.anim = null;
      this.current = null;
    };
    this.anim = anim;
  }

  get busy(): boolean {
    return this.anim !== null;
  }

  cancel(): void {
    this.anim?.cancel();
    this.anim = null;
    this.current = null;
  }
}

/**
 * Yeux qui changent de forme : ils se ferment en fente pendant que les
 * symboles défilent, puis s'ouvrent d'un coup avec un léger dépassement quand
 * le nouveau caractère se pose (~300 ms, voir `glyphs.ts`).
 */
export function morphEyes(eyes: HTMLElement, previous: Animation | null): Animation {
  const from = previous ? matrixOf(eyes).toString() : 'none';
  previous?.cancel();
  return eyes.animate(
    [
      { offset: 0, transform: from, easing: IN },
      { offset: 0.16, transform: 'scale(1.25,.12)', easing: IN_OUT },
      { offset: 0.34, transform: 'scale(.9,.7)', easing: IN_OUT },
      { offset: 0.5, transform: 'scale(1.12,.3)', easing: IN },
      { offset: 0.62, transform: 'scale(.86,1.28)', easing: OUT },
      { offset: 0.8, transform: 'scale(1.05,.95)', easing: IN_OUT },
      { offset: 1, transform: 'none' },
    ],
    { duration: 540 },
  );
}
