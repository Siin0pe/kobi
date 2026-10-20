// Défilement d'une réponse longue (KOB-163) : la bulle grandit jusqu'à sa
// hauteur maximale, puis son texte défile en douceur et suit la frappe tant
// que l'utilisateur ne remonte pas (molette, glisser, clavier). Revenu en bas,
// le suivi reprend. Un dégradé en haut et en bas signale la suite.

import { onFrame } from './ticker';

/** Écart (px) sous lequel on est « en bas ». */
const BOTTOM = 4;
/** Part de l'écart rattrapée à chaque image : un glissement doux, pas un saut. */
const EASE = 0.18;

export class Defilement {
  /** Suit le flux (en bas), tant que l'utilisateur ne remonte pas. */
  private follow = true;
  private stopTick: (() => void) | null = null;
  private drag: { y: number; top: number } | null = null;
  /** Le glisser a bougé : le clic du relâchement n'ouvre rien (KOB-246). */
  private glisse = false;
  /** Position et hauteur au défilement précédent : remonter arrête le suivi, seul un retour vers le bas le reprend. */
  private prevTop = 0;
  private prevHeight = 0;
  private readonly observer: ResizeObserver;
  private readonly mutations: MutationObserver;
  private instant = false;

  constructor(
    private readonly zone: HTMLElement,
    content: HTMLElement,
  ) {
    // La zone grandit jusqu'à sa hauteur maximale (taille), puis c'est le
    // contenu qui s'allonge (mots ajoutés par la frappe).
    this.observer = new ResizeObserver(() => this.poke());
    this.observer.observe(zone);
    this.mutations = new MutationObserver(() => this.poke());
    this.mutations.observe(content, { childList: true, subtree: true, characterData: true });
    zone.addEventListener('scroll', () => this.onScroll(), { passive: true });
    zone.addEventListener('wheel', (e) => {
      if (e.deltaY < 0 && zone.scrollTop > 0) this.follow = false;
    }, { passive: true });
    zone.addEventListener('pointerdown', (e) => {
      this.glisse = false;
      if (e.button !== 0 || zone.scrollHeight <= zone.clientHeight) return;
      // Bulle du mode simple : elle ne défile pas, rien à glisser (KOB-246).
      if (getComputedStyle(zone).overflowY === 'hidden') return;
      // Sur la barre de défilement, c'est elle qui défile : pas de glisser
      // en plus, qui la faisait trembler (KOB-246).
      const r = zone.getBoundingClientRect();
      const echelle = r.width / Math.max(1, zone.offsetWidth) || 1;
      if ((e.clientX - r.left) / echelle >= zone.clientLeft + zone.clientWidth) return;
      // Un bouton ou « N étapes » se clique : pas de glisser ni de capture du
      // pointeur, qui détournaient le clic quand la main bougeait (KOB-213).
      if ((e.target as Element | null)?.closest('button, a, summary, input, textarea, select')) return;
      this.drag = { y: e.clientY, top: zone.scrollTop };
      zone.setPointerCapture(e.pointerId);
    });
    zone.addEventListener('pointermove', (e) => {
      if (!this.drag) return;
      const scale = zone.getBoundingClientRect().height / Math.max(1, zone.offsetHeight);
      const top = this.drag.top - (e.clientY - this.drag.y) / (scale || 1);
      if (Math.abs(e.clientY - this.drag.y) > 4) this.glisse = true;
      if (top < zone.scrollTop) this.follow = false;
      zone.scrollTop = top;
    });
    const release = (): void => {
      this.drag = null;
    };
    zone.addEventListener('pointerup', release);
    zone.addEventListener('pointercancel', release);
    // Après un glisser pour défiler, le relâchement n'est pas un clic (qui
    // ouvrait la vue agrandie, KOB-246).
    zone.addEventListener(
      'click',
      (e) => {
        if (!this.glisse) return;
        this.glisse = false;
        e.stopPropagation();
        e.preventDefault();
      },
      true,
    );
    this.paintFades();
  }

  /** Réduire les animations : le suivi saute en bas sans glisser. */
  setInstant(instant: boolean): void {
    this.instant = instant;
  }

  /** Revient en bas et reprend le suivi (ouverture de la vue agrandie). */
  suivreBas(): void {
    this.follow = true;
    this.zone.scrollTop = this.zone.scrollHeight;
    this.paintFades();
  }

  /** Remonté par l'utilisateur ? (tests) */
  get following(): boolean {
    return this.follow;
  }

  destroy(): void {
    this.observer.disconnect();
    this.mutations.disconnect();
    this.stopTick?.();
    this.stopTick = null;
  }

  /** Le contenu a changé de taille : on glisse vers le bas si on suit. */
  private poke(): void {
    this.paintFades();
    // Widget simple (KOB-169) : la bulle montre le début de la réponse ; la
    // suite se lit dans la vue agrandie.
    if (this.zone.closest('.affichage-simple .card-answer')) {
      this.zone.scrollTop = 0;
      return;
    }
    if (!this.follow || this.stopTick) return;
    if (this.instant) {
      this.zone.scrollTop = this.zone.scrollHeight;
      return;
    }
    this.stopTick = onFrame(() => this.glide());
  }

  private glide(): boolean {
    const z = this.zone;
    const target = z.scrollHeight - z.clientHeight;
    const gap = target - z.scrollTop;
    if (!this.follow || gap <= 1) {
      this.stopTick = null;
      return false;
    }
    z.scrollTop += Math.max(1, gap * EASE);
    return true;
  }

  private onScroll(): void {
    const z = this.zone;
    const atBottom = z.scrollTop + z.clientHeight >= z.scrollHeight - BOTTOM;
    // Le suivi ne fait que descendre : un pas vers le haut vient de
    // l'utilisateur (molette, barre, clavier), sauf si le contenu a rétréci.
    const remonte = z.scrollTop < this.prevTop - 0.5 && z.scrollHeight >= this.prevHeight;
    const descend = z.scrollTop >= this.prevTop;
    this.prevTop = z.scrollTop;
    this.prevHeight = z.scrollHeight;
    // Remonté à la main, même de quelques pixels : on cesse de suivre ;
    // revenu en bas en descendant : on reprend (KOB-246).
    if (remonte) this.follow = false;
    else if (atBottom && descend) this.follow = true;
    this.paintFades();
  }

  private paintFades(): void {
    const z = this.zone;
    const up = z.scrollTop > 2;
    const down = z.scrollTop + z.clientHeight < z.scrollHeight - BOTTOM;
    z.classList.toggle('peut-monter', up);
    z.classList.toggle('peut-descendre', down);
  }
}
