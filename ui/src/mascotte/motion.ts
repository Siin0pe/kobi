// Changement d'état sans saut. Les animations CSS de la pilule repartent de
// zéro quand la classe d'état change : la pilule sauterait de sa pose courante
// (penchée, en plein rebond, affaissée…) à la première image de la nouvelle
// animation. On mesure la pose avant et après le changement, et une enveloppe
// parente rattrape l'écart en douceur.

const EASING = 'cubic-bezier(.2,.7,.3,1)';

function matrixOf(node: Element): DOMMatrix {
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
