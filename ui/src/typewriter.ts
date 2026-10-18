// Texte qui se tape. Le texte cible peut grandir à tout moment (réponse en
// flux) : on tape le retard à vitesse régulière, en accélérant quand il
// s'accumule. La frappe passe par la boucle d'animation partagée et ne tourne
// que tant qu'il reste à taper.
//
// Chaque mot est un élément : il arrive avec un petit mouvement, et les mots
// qui méritent l'œil (chiffres, chemins, noms de fichiers) sont mis en valeur.
// Le DOM est complété au fil de la frappe, jamais reconstruit tant que le
// texte ne fait que grandir.

import { onFrame } from './ticker';

/** Retard (en caractères) au-delà duquel la vitesse augmente. */
const BACKLOG = 40;

/** Mots mis en valeur : chiffres, chemins, noms de fichiers (`refresh.ts`). */
export const HIGHLIGHT = /\d|[\\/]|\w\.[a-z]{1,5}\b/i;

export class Typewriter {
  private target: string[] = [];
  private shown = 0;
  private carry = 0;
  private last = 0;
  private stopTick: (() => void) | null = null;
  private instant = false;
  /** Texte déjà dans le DOM. */
  private rendered = '';
  /** Mot en cours de frappe. */
  private word: HTMLElement | null = null;

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

  /** Interruption (Échap) : le texte s'arrête là où en est la frappe. */
  freeze(): void {
    this.target = this.target.slice(0, this.shown);
    this.stop();
  }

  stop(): void {
    this.stopTick?.();
    this.stopTick = null;
  }

  private schedule(): void {
    if (this.done || this.instant) {
      this.stop();
      if (this.done) this.onDone?.();
      return;
    }
    if (this.stopTick) return;
    this.last = performance.now();
    this.stopTick = onFrame(this.tick);
  }

  private readonly tick = (now: number): boolean => {
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
      this.stopTick = null;
      this.onDone?.();
      return false;
    }
    return true;
  };

  private paint(): void {
    const text = this.target.slice(0, this.shown).join('');
    if (text === this.rendered) return;
    if (!text.startsWith(this.rendered)) {
      this.node.replaceChildren();
      this.rendered = '';
      this.word = null;
    }
    for (const ch of text.slice(this.rendered.length)) {
      if (/\s/.test(ch)) {
        this.closeWord();
        this.node.append(ch);
        continue;
      }
      if (!this.word) {
        this.word = document.createElement('span');
        this.word.className = this.instant ? 'w' : 'w w-in';
        this.node.append(this.word);
      }
      this.word.textContent += ch;
    }
    this.rendered = text;
    if (this.done) this.closeWord();
  }

  private closeWord(): void {
    if (this.word && HIGHLIGHT.test(this.word.textContent ?? '')) this.word.classList.add('hl');
    this.word = null;
  }
}
