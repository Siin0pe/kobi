// Images par seconde (KOB-200), et repos économe (KOB-146) : un métronome
// pour toute la scène.
//
// Par défaut, tout bouge à 60 images/s (réglage « Images par seconde » de
// l'app : 5, 15, 30, 60, 120 ou suivre l'écran, cadence.ts). Sous la fréquence
// de l'écran, chaque animation de la scène (boucles et animations finies,
// transitions, animations Web) passe en escalier : `steps(n)` sur son effet,
// un pas tous les 1/rythme de seconde, départ posé sur une grille commune.
// Toutes changent sur la même image, et le navigateur les anime seul : aucun
// appel par image, aucun style recalculé hors des pas (KOB-240 : l'ancienne
// horloge les faisait avancer elle-même à chaque image, d'où un repeint
// continu). À la fréquence de l'écran (ou au-dessus), rien n'est touché.
//
// « Économie d'énergie » (désactivée par défaut) reprend l'ancien repos
// économe : au repos prolongé, 3 images/s, puis 2 après une minute. Elle ne
// fait que plafonner la cadence réglée.
//
// Les gestes de la vie au repos (lettre qui sautille, bâillement, regard) sont
// des animations Web lancées par `el.animate()` : elles suivent aussi le
// métronome, puis sont relâchées à leur fin (leur `onfinish` se déclenche).
// La boucle partagée (ticker.ts : lettres qui se brouillent, frappe des
// bulles) prend le même rythme quand elle tourne.

import { setFrameInterval } from './ticker';

/** Images par seconde de l'économie d'énergie au repos, puis au sommeil prolongé. */
export const ECONOMY_FPS = 3;
export const DEEP_FPS = 2;
/** Délai avant le sommeil prolongé. */
export const DEEP_AFTER_MS = 60_000;

/** Métronomes actifs : ils adoptent les animations Web créées dans leur scène. */
const active = new Set<Metronome>();
const nativeAnimate = Element.prototype.animate;
Element.prototype.animate = function (this: Element, ...args: Parameters<Element['animate']>): Animation {
  const anim = nativeAnimate.apply(this, args);
  for (const m of active) m.adopt(this, anim);
  return anim;
};

function isLoop(a: Animation): boolean {
  return a instanceof CSSAnimation && a.effect?.getTiming().iterations === Infinity;
}

/**
 * Tout ce qui bouge sous `root` : boucles et animations finies, transitions
 * CSS et animations Web (KOB-200 : « tout suit ce nombre » ; l'essai Xvfb
 * montrait des rafales à 60 images/s à 5 images/s réglées). Pseudo-éléments
 * compris : l'ondulation du mot vit dans `::before`/`::after`, et WebKit ne
 * les rend pas par `root.getAnimations({ subtree: true })` ;
 * `document.getAnimations()` les donne partout.
 */
function enCours(root: Element): Animation[] {
  return document.getAnimations().filter((a) => {
    const cible = (a.effect as KeyframeEffect | null)?.target;
    return a.playState === 'running' && !!cible && root.contains(cible);
  });
}

export class Metronome {
  /** Animations tenues par le métronome, avec leur courbe et leur vitesse d'origine. */
  private readonly held = new Map<Animation, { easing: string; vitesse: number }>();
  private slow = 0;
  /** Cadence réglée, 0 : celle de l'écran (le navigateur anime lui-même). */
  private cadence = 0;
  /** Repos économe (« Économie d'énergie » au repos prolongé). */
  private economie = false;
  /** Geste en cours (scène de la veille, ballottement) : voir `setLively`. */
  private lively = false;
  private suspended = false;
  private fps = ECONOMY_FPS;
  private readonly onStart = (): void => {
    // Animations remplacées (changement d'état) ou finies : oubliées, avec
    // leurs nœuds (KOB-264).
    for (const a of [...this.held.keys()]) if (a.playState === 'idle' || a.playState === 'finished') this.held.delete(a);
    if (!this.rate || this.geste()) return;
    // Une animation ou une transition qui démarre (fiche, « z », couche d'une
    // lettre, regard) suit le métronome.
    for (const a of enCours(this.root)) this.hold(a);
  };

  /** @param root élément dont les boucles sont tenues ; `onRate` reçoit le rythme (tests). */
  constructor(
    private readonly root: HTMLElement,
    private readonly onRate?: (fps: number) => void,
  ) {
    root.addEventListener('animationstart', this.onStart);
    root.addEventListener('transitionrun', this.onStart);
  }

  /** Images par seconde imposées, 0 : celles de l'écran (le navigateur anime). */
  get rate(): number {
    if (!this.economie) return this.cadence;
    return this.cadence ? Math.min(this.cadence, this.fps) : this.fps;
  }

  /** Animation Web créée dans la scène : elle suit le métronome. */
  adopt(el: Element, anim: Animation): void {
    if (this.rate && !this.geste() && this.root.contains(el)) this.hold(anim);
  }

  /**
   * Images par seconde réglées (KOB-200), 0 pour celles de l'écran.
   * Appliqué à chaud : les boucles sont tenues ou relâchées sur-le-champ.
   */
  setCadence(fps: number): void {
    if (fps === this.cadence) return;
    this.cadence = fps;
    this.appliquer();
  }

  /** Entrée (ou sortie) du repos économe (« Économie d'énergie » activée). */
  setActive(on: boolean): void {
    if (on === this.economie) return;
    this.economie = on;
    window.clearTimeout(this.slow);
    this.fps = ECONOMY_FPS;
    if (on) this.slow = window.setTimeout(() => this.setRate(DEEP_FPS), DEEP_AFTER_MS);
    this.appliquer();
  }

  /** Fenêtre masquée ou scène repliée : plus aucune image, boucles tenues en l'état. */
  setSuspended(suspended: boolean): void {
    this.suspended = suspended;
    this.run();
  }

  destroy(): void {
    this.setActive(false);
    this.setCadence(0);
    this.root.removeEventListener('animationstart', this.onStart);
    this.root.removeEventListener('transitionrun', this.onStart);
  }

  /**
   * Geste au repos économe (scène de la veille, ballottement, KOB-149) : à
   * la cadence de l'écran, il se joue à pleine vitesse (ses animations Web ne
   * sont plus tenues, la boucle partagée tourne à chaque image), mais les
   * boucles de fond restent au rythme du repos économe (KOB-170).
   */
  setLively(on: boolean): void {
    if (on === this.lively) return;
    this.lively = on;
    this.frameInterval();
  }

  /** Geste à pleine vitesse pendant le repos économe, sur un écran laissé à lui-même. */
  private geste(): boolean {
    return this.lively && this.economie && this.cadence === 0;
  }

  /** Tient ou relâche tout selon le rythme courant. */
  private appliquer(): void {
    if (this.rate) {
      active.add(this);
      // Les gestes à pleine vitesse (repos économe) ne sont pas tenus, les boucles si.
      for (const a of enCours(this.root)) if (isLoop(a) || !this.geste()) this.hold(a);
      // Déjà tenues : elles prennent le nouveau pas.
      for (const a of [...this.held.keys()]) this.hold(a);
      this.run();
    } else {
      active.delete(this);
      for (const [a, easing] of this.held) this.release(a, easing);
      this.held.clear();
    }
    this.frameInterval();
    this.onRate?.(this.rate);
  }

  /** La boucle partagée suit le métronome, sauf pendant un geste à pleine vitesse. */
  private frameInterval(): void {
    setFrameInterval(this.rate && !this.geste() ? 1000 / this.rate : 0);
  }

  private setRate(fps: number): void {
    this.fps = fps;
    for (const a of [...this.held.keys()]) this.hold(a);
    this.frameInterval();
    this.onRate?.(this.rate);
  }

  /**
   * Tient une animation au rythme réglé, sans la faire avancer nous-mêmes
   * (KOB-200, sans la boucle par image de KOB-240) : son temps passe en
   * escalier, `steps(n)` sur l'effet, un pas tous les 1/rythme de seconde ;
   * les courbes de ses images clés restent les siennes. Son départ est posé
   * sur la grille commune : toutes avancent sur la même image. Ensuite, le
   * navigateur l'anime seul, sans un seul appel par image.
   */
  private hold(a: Animation): void {
    const effet = a.effect;
    if (!effet) return;
    if (!this.held.has(a)) this.held.set(a, { easing: String(effet.getTiming().easing ?? 'linear'), vitesse: a.playbackRate });
    const origine = this.held.get(a)!;
    const pas = 1000 / this.rate;
    const duree = Number(effet.getTiming().duration);
    if (!(duree > 0)) return;
    const n = Math.max(1, Math.round(duree / pas));
    effet.updateTiming({ easing: `steps(${n})` });
    // Chaque pas dure exactement 1/rythme de seconde : une durée qui n'est pas
    // un multiple du pas (1 100 ms à 5 images/s) est jouée à peine plus vite
    // ou plus lentement, sinon ses pas tomberaient hors de la grille commune
    // et ajouteraient des images (essai WebKitGTK).
    a.playbackRate = origine.vitesse * (duree / (n * pas));
    // Départ sur la grille commune. Une animation qui vient d'être créée n'a
    // pas encore de départ (`startTime` nul, en attente) : WebKit en donne
    // ainsi à l'adoption, et chacune partait alors à son instant, d'où des
    // pas décalés et trois fois plus d'images (KOB-219). On l'aligne dès
    // qu'elle est prête.
    // C'est le début réel qui se cale, délai compris : les « z » partent
    // l'un après l'autre (délais décalés), et leurs pas tombaient chacun à
    // leur heure (KOB-219).
    const aligner = (): void => {
      if (a.startTime === null || !this.held.has(a)) return;
      const delai = Number(effet.getTiming().delay ?? 0) / (a.playbackRate || 1);
      a.startTime = Math.round((Number(a.startTime) + delai) / pas) * pas - delai;
    };
    if (a.startTime === null) a.ready.then(aligner, () => {});
    else aligner();
    if (this.suspended && a.playState === 'running') a.pause();
  }

  /** Rend à une animation sa courbe d'origine : le navigateur l'anime à la cadence de l'écran. */
  private release(a: Animation, { easing, vitesse }: { easing: string; vitesse: number }): void {
    a.effect?.updateTiming({ easing });
    a.playbackRate = vitesse;
    if (a.playState === 'paused') a.play();
  }

  /** Fenêtre cachée : les animations tenues s'arrêtent là où elles sont. */
  private run(): void {
    for (const a of this.held.keys()) {
      if (this.suspended && a.playState === 'running') a.pause();
      else if (!this.suspended && a.playState === 'paused') a.play();
    }
  }
}
