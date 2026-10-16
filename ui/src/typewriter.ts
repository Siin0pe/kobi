// Texte qui se tape. Le texte cible peut grandir à tout moment (réponse en
// flux) : on tape le retard à vitesse régulière, en accélérant quand il
// s'accumule. La boucle d'animation ne tourne que tant qu'il reste à taper.

/** Retard (en caractères) au-delà duquel la vitesse augmente. */
const BACKLOG = 40;

export class Typewriter {
  private target: string[] = [];
  private shown = 0;
  private carry = 0;
  private last = 0;
  private frame = 0;
  private instant = false;

  /**
   * @param node élément dont le texte est tapé
   * @param cps vitesse de base en caractères par seconde
   * @param onDone appelé quand tout le texte cible est affiché
   */
  constructor(
    private readonly node: Element,
    private readonly cps: number,
    private readonly onDone?: () => void,
  ) {}

  /** Change le texte cible. Si l'ancien texte en est le début, la frappe continue. */
  set(text: string): void {
    const next = Array.from(text);
    const keep = this.shown <= next.length && next.slice(0, this.shown).join('') === this.target.slice(0, this.shown).join('');
    this.target = next;
    if (!keep) {
      this.shown = 0;
      this.carry = 0;
    }
    if (this.instant) this.shown = next.length;
    this.paint();
    this.schedule();
  }

  /** Affiche tout immédiatement (réduire les animations). */
  setInstant(instant: boolean): void {
    this.instant = instant;
    if (instant) {
      this.shown = this.target.length;
      this.paint();
    }
    this.schedule();
  }

  get done(): boolean {
    return this.shown >= this.target.length;
  }

  stop(): void {
    cancelAnimationFrame(this.frame);
    this.frame = 0;
  }

  private schedule(): void {
    if (this.done || this.instant) {
      this.stop();
      if (this.done) this.onDone?.();
      return;
    }
    if (this.frame) return;
    this.last = performance.now();
    this.frame = requestAnimationFrame(this.tick);
  }

  private readonly tick = (now: number): void => {
    this.frame = 0;
    // Au retour d'une fenêtre masquée, pas de rattrapage brutal.
    const dt = Math.min(now - this.last, 100) / 1000;
    this.last = now;
    const backlog = this.target.length - this.shown;
    const speed = backlog > BACKLOG ? (this.cps * backlog) / BACKLOG : this.cps;
    this.carry += speed * dt;
    const step = Math.floor(this.carry);
    if (step > 0) {
      this.carry -= step;
      this.shown = Math.min(this.target.length, this.shown + step);
      this.paint();
    }
    if (this.done) {
      this.onDone?.();
      return;
    }
    this.frame = requestAnimationFrame(this.tick);
  };

  private paint(): void {
    const text = this.target.slice(0, this.shown).join('');
    if (this.node.textContent !== text) this.node.textContent = text;
  }
}
