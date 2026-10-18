// Kobi, style Glyphe : un visage fait de lettres. Port du prototype
// docs/design/mascotte/Glyphe.dc.html, poussé plus loin (KOB-137).
//
// Calques, du plus extérieur au plus intérieur, chacun avec son propre
// mouvement pour qu'ils se composent sans se battre :
//
//   .drag     ballotte quand on déplace la fenêtre (ressort, inertie)
//   .lean     penché vers le curseur (transition CSS)
//   .voice    suit le volume du micro en écoute
//   .react    gestes ponctuels : anticipation, saut, rebond (motion.ts)
//   .inertia  absorbe la pose de la pilule au changement d'état
//   .pill     boucle de l'état (CSS)
//   .gaze     regard : état, curseur, coups d'œil (transition CSS)
//   .eyes     morphing des yeux ; chaque œil `.g` cligne
//
// Un changement d'état est chorégraphié : l'ancien état s'efface pendant
// l'anticipation (le corps s'écrase, les yeux se ferment en fente, les
// symboles défilent), puis le nouveau prend le relais au « décollage »
// (classe d'état, effets qui surgissent, lettres qui se posent en rebondissant).
//
// Sobriété : hors des transitions et des gestes, seules les boucles CSS de
// l'état tournent ; la vie (coups d'œil, bâillements…) est faite de minuteries
// espacées. Fenêtre masquée ou animations réduites : aucune minuterie.

import type { MascotData, MascotState } from '../states';
import { onFrame } from '../ticker';
import { Blinker, Scrambler, type Eyes } from './glyphs';
import { GESTURES, Gestures, Inertia, morphEyes, Sway, type Gesture } from './motion';
import { Orbit } from './orbit';
import { Travail, WORK_EYES } from './travail';
import './kobi.css';

interface Look {
  eye: string;
  word: string;
  /** Bouche au repos. */
  mouth: string;
  /** Geste d'entrée dans l'état. */
  enter: Gesture;
  /** Direction du regard (px), vers la carte de l'état. */
  gaze: readonly [number, number];
}

export const LOOKS: Record<MascotState, Look> = {
  idle: { eye: '–', word: 'zzz', mouth: '', enter: 'settle', gaze: [0, 0] },
  listen: { eye: 'O', word: 'oui ?', mouth: '', enter: 'startle', gaze: [-14, 0] },
  think: { eye: '@', word: 'hmm…', mouth: '', enter: 'ponder', gaze: [18, -18] },
  write: { eye: 'o', word: 'noté', mouth: '', enter: 'lean', gaze: [-18, 16] },
  answer: { eye: '^', word: 'alors', mouth: 'o', enter: 'speak', gaze: [-10, -4] },
  suggest: { eye: '*', word: 'idée !', mouth: '', enter: 'pop', gaze: [-12, -6] },
  done: { eye: '^', word: 'fait !', mouth: '', enter: 'cheer', gaze: [0, 0] },
  error: { eye: 'x', word: 'oups', mouth: '~', enter: 'recoil', gaze: [0, 6] },
  work: { eye: WORK_EYES.fouille, word: 'occupé', mouth: '', enter: 'lean', gaze: [-10, 16] },
};

/** États où le survol fait sourire Kobi. */
const HAPPY_ON_HOVER: ReadonlySet<MascotState> = new Set(['idle', 'listen', 'answer', 'suggest', 'done']);

/** États où Kobi cligne des yeux. */
const BLINKS: ReadonlySet<MascotState> = new Set(['listen', 'write', 'answer', 'done']);

/** Effet visible dans chaque état (symboles autour de la pilule). */
const FX: Partial<Record<MascotState, string>> = {
  idle: '.zz',
  listen: '.waves',
  think: '.orbit',
  suggest: '.bang',
  done: '.bang',
  error: '.drop',
  work: '.travail',
};

/** Délai avant le sommeil profond (veille économe, voir kobi.css). */
export const DEEP_SLEEP_MS = 20_000;
/**
 * États de repos qui passent en mode économe : la veille, et la prise de
 * notes, repos de Kobi pendant un enregistrement (des heures).
 */
const RESTING: ReadonlySet<MascotState> = new Set(['idle', 'write']);

/** Une image de bouche toutes les 110 ms quand Kobi parle. */
const MOUTH_MS = 110;
/** Bouche qui parle sans texte (démonstration) : rythme du prototype. */
const MOUTH = ['o', 'O', 'o', '-', 'O', 'o'];
const VOWELS = /[aeiouyàâéèêëîïôöùûü]/i;

const rand = (min: number, max: number): number => min + Math.random() * (max - min);

/**
 * Répertoire de la veille (KOB-149) : petites scènes rares, tirées au hasard,
 * jamais deux fois la même d'affilée. Le poids règle la fréquence.
 */
export const IDLE_EVENTS = {
  glitch: 3,
  peek: 2,
  doze: 2,
  yawn: 2,
  shiver: 1,
  look: 1,
  whistle: 2,
  juggle: 1,
  tap: 2,
  sneeze: 1,
  hiccup: 1,
  wink: 2,
  snore: 1,
  hum: 2,
  reach: 2,
  roll: 1,
  twirl: 1,
  dream: 1,
} as const;
export type IdleEvent = keyof typeof IDLE_EVENTS;

/** Trajet d'un symbole qui s'envole, depuis le centre de la pilule (px). */
interface Flight {
  x: number;
  y: number;
  dx: number;
  dy: number;
  size?: number;
  duration?: number;
  rotate?: number;
}

function el(tag: string, className?: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Minuteries d'un état, toutes annulées d'un coup (changement d'état, pause). */
class Timers {
  private readonly ids = new Set<number>();

  after(ms: number, fn: () => void): void {
    const id = window.setTimeout(() => {
      this.ids.delete(id);
      fn();
    }, ms);
    this.ids.add(id);
  }

  clear(): void {
    this.ids.forEach((id) => window.clearTimeout(id));
    this.ids.clear();
  }
}

/** La mascotte, posée par la scène dans la partie droite (x ≥ 400 px). */
export class Kobi {
  readonly root: HTMLElement;
  private readonly body: HTMLElement;
  private readonly drag: HTMLElement;
  private readonly lean: HTMLElement;
  private readonly voice: HTMLElement;
  private readonly pill: HTMLElement;
  private readonly gaze: HTMLElement;
  private readonly eyesBox: HTMLElement;
  private readonly eyes: HTMLElement[];
  private readonly mouth: HTMLElement;
  private readonly word: HTMLElement;
  private readonly burst: HTMLElement;
  /** Anneau rouge de l'enregistrement, autour de la pilule. */
  private readonly ring: HTMLElement;
  private readonly orbit = new Orbit();
  private readonly travail = new Travail();
  private readonly inertia: Inertia;
  /** Ressort du calque `.drag` (déplacement de la fenêtre). */
  private readonly sway: Sway;
  private readonly gestures: Gestures;
  private readonly scrambler: Scrambler;
  private readonly blinker: Blinker;
  /** Minuteries de la vie de l'état courant. */
  private readonly life = new Timers();
  private state: MascotState | null = null;
  /** État dont la classe est posée (en retard sur `state` pendant l'anticipation). */
  private shown: MascotState | null = null;
  private data: MascotData = {};
  private reduced = false;
  private paused = false;
  private deep = false;
  /** Entrée et sortie du repos économe (la scène met ses bulles au même rythme). */
  onDeepChange: ((deep: boolean) => void) | null = null;
  /**
   * Une scène de la veille ou un ballottement se joue (`true`) ou se termine :
   * pendant ce temps, la scène relâche son métronome (60 images/s), sans
   * quitter le repos économe.
   */
  onLivelyChange: ((lively: boolean) => void) | null = null;
  /** Raisons d'être à 60 images/s pendant le repos économe. */
  private readonly lively = new Set<string>();
  private switchTimer = 0;
  private deepTimer = 0;
  private morph: Animation | null = null;
  private fading: Animation[] = [];

  // Parole (Répond) : caractères encore à « prononcer ».
  private speech = '';
  private mouthTimer = 0;
  private mouthStep = 0;

  // Volume du micro (Écoute).
  private levelTarget = 0;
  private level = 0;
  private levelStop: (() => void) | null = null;

  // Curseur.
  private pointer: { x: number; y: number } | null = null;
  private pointerStop: (() => void) | null = null;
  private pointerIdle = 0;
  private lastBoop = 0;
  /** Réveillé par le survol pendant la veille. */
  private awake = false;
  private glance: readonly [number, number] | null = null;
  /** Point regardé à la demande (curseur de la bulle d'écriture, KOB-147). */
  private target: { x: number; y: number } | null = null;
  private lastIdleEvent = '';
  private readonly cleanups: Array<() => void> = [];

  constructor(reducedMotion: boolean) {
    this.reduced = reducedMotion;
    this.root = el('div', 'kobi m-idle');
    this.root.setAttribute('aria-hidden', 'true');

    this.body = el('div', 'body');
    const zz = el('div', 'fx zz');
    zz.style.cssText = 'right:6px;top:-10px';
    zz.append(el('span', '', 'z'), el('span', '', 'z'), el('span', '', 'z'));
    const waves = el('div', 'fx waves');
    waves.style.cssText = 'left:-64px;top:48px';
    waves.append(el('span', '', '('), el('span', '', '('), el('span', '', '('));
    const bang = el('div', 'fx bang', '!');
    bang.style.cssText = 'right:-78px;top:-30px';
    const drop = el('div', 'fx drop', ',');
    drop.style.cssText = 'right:36px;top:4px';
    this.burst = el('div', 'burst');

    this.drag = el('div', 'drag');
    this.lean = el('div', 'lean');
    this.voice = el('div', 'voice');
    const react = el('div', 'react');
    this.ring = el('div', 'rec-ring');
    const inertia = el('div', 'inertia');
    this.pill = el('div', 'pill');
    this.gaze = el('div', 'gaze');
    this.eyesBox = el('div', 'eyes');
    this.eyes = [el('span', 'g'), el('span', 'g')];
    this.eyesBox.append(...this.eyes);
    this.gaze.append(this.eyesBox);
    this.mouth = el('div', 'mouth');
    this.pill.append(this.ring, this.gaze, this.mouth);
    inertia.append(this.pill);
    react.append(inertia);
    this.voice.append(react);
    this.lean.append(this.voice);
    this.drag.append(this.lean);
    this.sway = new Sway(this.drag);
    this.inertia = new Inertia(inertia, this.pill);
    this.sway.onRest = () => this.setLively('sway', false);
    this.gestures = new Gestures(react);

    this.body.append(this.orbit.root, this.travail.root, zz, waves, bang, drop, this.drag, this.burst);
    this.word = el('div', 'word');
    this.root.append(this.body, this.word);
    this.scrambler = new Scrambler(this.eyes, this.word);
    this.scrambler.setInstant(this.reduced);
    this.blinker = new Blinker((closed) => this.blink(closed));
    this.listenPointer();
    this.listenWindow();
    this.applyFlags();
  }

  /** Animations permises et fenêtre visible. */
  private get live(): boolean {
    return !this.reduced && !this.paused;
  }

  setState(state: MascotState, data: Readonly<MascotData> = {}): void {
    if (state === this.state) {
      this.setData(data);
      return;
    }
    const previous = this.state;
    this.state = state;
    this.data = { ...data };
    const look = LOOKS[state];
    this.root.dataset.state = state;

    this.life.clear();
    this.endScene();
    delete this.root.dataset.pinched;
    this.awake = false;
    this.glance = null;
    this.stopLevel();
    this.scrambler.show(look.eye, look.word);
    this.speech = state === 'answer' ? (data.answer ?? '').slice(0, 160) : '';
    window.clearTimeout(this.switchTimer);

    const animated = previous !== null && this.live;
    if (!animated) {
      this.switchTo(state, false);
      return;
    }
    // Anticipation : le corps s'écrase, les yeux se ferment en fente, l'effet
    // de l'ancien état s'efface ; le nouvel état se pose au décollage.
    this.gestures.play(look.enter);
    this.morph = morphEyes(this.eyesBox, this.morph);
    this.squashWord();
    if (state !== 'done') this.sparks(state === 'error' ? 4 : 7);
    this.fadeOut(this.shown);
    this.aim();
    this.switchTimer = window.setTimeout(() => this.switchTo(state, true), GESTURES[look.enter].launch);
  }

  /** Pose la classe de l'état et lance sa vie. */
  private switchTo(state: MascotState, animated: boolean): void {
    this.switchTimer = 0;
    this.fading.forEach((a) => a.cancel());
    this.fading = [];
    this.shown = state;
    window.clearTimeout(this.deepTimer);
    this.deep = false;
    this.inertia.carry(() => this.applyFlags(), animated);
    this.mouth.textContent = LOOKS[state].mouth;
    this.mouthStep = 0;

    if (state === 'think') this.orbit.start(this.reduced);
    else this.orbit.stop();
    if (state === 'work') this.setTool(this.data.tool, false);
    if (this.paused) this.orbit.pause(true);
    if (animated) {
      this.popIn(state);
      if (state === 'done') this.confetti();
    }
    this.aim();
    this.syncTimers();
    this.startLife();
    this.scrambler.resync();
    if (RESTING.has(state)) this.deepTimer = window.setTimeout(() => this.deepSleep(), DEEP_SLEEP_MS);
  }

  /** Classes de la racine : état affiché et réglages. */
  private applyFlags(): void {
    const c = this.root.classList;
    for (const name of Array.from(c)) if (name.startsWith('m-') && name !== `m-${this.shown}`) c.remove(name);
    if (this.shown) c.add(`m-${this.shown}`);
    c.toggle('reduced-motion', this.reduced);
    c.toggle('paused', this.paused);
    if (c.contains('deep') !== this.deep) {
      c.toggle('deep', this.deep);
      this.onDeepChange?.(this.deep);
    }
  }

  /** Fin d'une anticipation en cours : l'état demandé se pose tout de suite. */
  private flushSwitch(): void {
    if (!this.switchTimer || !this.state) return;
    window.clearTimeout(this.switchTimer);
    this.switchTo(this.state, false);
  }

  setData(data: Readonly<MascotData>): void {
    const before = this.data;
    this.data = { ...data };
    // L'outil change même sans animation : yeux et icône suivent (réduire les animations).
    if (this.state === 'work' && data.tool !== before.tool) this.setTool(data.tool, true);
    if (!this.live) return;
    switch (this.state) {
      case 'answer': {
        const prev = before.answer ?? '';
        const next = data.answer ?? '';
        if (next.length > prev.length && next.startsWith(prev)) {
          const added = next.slice(prev.length);
          this.speech = (this.speech + added).slice(-160);
          this.syncTimers();
          // Fin de phrase : petit hochement.
          if (/[.?!…]/.test(added) && !this.gestures.busy) this.life.after(160, () => this.gestures.play('nod'));
        }
        break;
      }
      case 'think': {
        const found = (s: MascotData) => (s.sources ?? []).filter((x) => x.found).length;
        if (found(data) > found(before)) {
          this.flash(['!', '!'], 320);
          if (!this.gestures.busy) this.gestures.play('aha');
        }
        break;
      }
      case 'write':
        if ((data.notes ?? '').length > (before.notes ?? '').length && !this.gestures.busy) this.gestures.play('nod');
        break;
      case 'error':
        if (data.error?.message !== before.error?.message && !this.gestures.busy) this.gestures.play('flinch');
        break;
    }
  }

  /** Je m'en occupe : le geste et les yeux suivent l'outil en cours. */
  private setTool(tool: MascotData['tool'], animated: boolean): void {
    const changed = this.travail.set(tool);
    this.root.dataset.outil = this.travail.outil;
    if (!changed && animated) return;
    this.scrambler.show(WORK_EYES[tool ?? 'fouille'], LOOKS.work.word);
    if (animated && this.live && !this.gestures.busy) this.gestures.play('nod');
  }

  /**
   * Réunion enregistrée : un anneau rouge pulse autour de la pilule, quel que
   * soit l'état (Kobi peut répondre ou réfléchir pendant qu'il enregistre).
   * En pause, l'anneau devient gris et pointillé.
   */
  setRecording(recording: { paused: boolean } | null): void {
    const was = this.root.classList.contains('recording');
    this.root.classList.toggle('recording', recording !== null);
    this.root.classList.toggle('rec-paused', Boolean(recording?.paused));
    if (recording && !was && this.live) {
      this.ring.animate(
        [
          { transform: 'scale(1.35)', opacity: 0 },
          { transform: 'scale(.96)', opacity: 1, offset: 0.6 },
          { transform: 'scale(1)', opacity: 1 },
        ],
        { duration: 520, easing: 'cubic-bezier(.2,.8,.3,1)', composite: 'replace' },
      );
    }
  }

  /** Une note vient d'être prise : l'anneau s'illumine, Kobi acquiesce. */
  noteAdded(): void {
    if (!this.live || !this.root.classList.contains('recording')) return;
    this.ring.animate(
      [
        { boxShadow: '0 0 0 0 rgba(255,59,48,.75)', transform: 'scale(1)' },
        { boxShadow: '0 0 0 14px rgba(255,59,48,0)', transform: 'scale(1.05)', offset: 0.7 },
        { boxShadow: '0 0 0 0 rgba(255,59,48,0)', transform: 'scale(1)' },
      ],
      { duration: 700, easing: 'ease-out' },
    );
    if (!this.gestures.busy) this.gestures.play('nod');
  }

  /**
   * Volume du micro, de 0 à 1 (événement `niveau` de la coque, ou démo). Pris
   * en compte en écoute seulement ; sans nouvelle valeur, il retombe seul.
   */
  setLevel(level: number): void {
    this.levelTarget = Math.max(0, Math.min(1, level));
    if (this.state === 'listen' && this.live && !this.levelStop) this.levelStop = onFrame(() => this.levelTick());
  }

  private levelTick(): boolean {
    const up = this.levelTarget > this.level;
    this.level += (this.levelTarget - this.level) * (up ? 0.45 : 0.14);
    this.levelTarget *= 0.9;
    const v = this.level;
    this.voice.style.transform = `translateY(${(-v * 10).toFixed(2)}px) scale(${(1 + v * 0.05).toFixed(4)},${(1 + v * 0.09).toFixed(4)})`;
    this.body.style.setProperty('--lv', v.toFixed(3));
    if (v < 0.005 && this.levelTarget < 0.005) {
      this.levelStop = null;
      this.resetLevel();
      return false;
    }
    return true;
  }

  private stopLevel(): void {
    this.levelStop?.();
    this.levelStop = null;
    this.resetLevel();
  }

  private resetLevel(): void {
    this.level = 0;
    this.levelTarget = 0;
    this.voice.style.transform = '';
    this.body.style.removeProperty('--lv');
  }

  setPaused(paused: boolean): void {
    if (paused === this.paused) return;
    this.flushSwitch();
    this.paused = paused;
    this.applyFlags();
    this.orbit.pause(paused || this.reduced);
    this.inertia.pause(paused);
    this.refreshLife();
  }

  setReducedMotion(reduced: boolean): void {
    if (reduced === this.reduced) return;
    this.flushSwitch();
    this.reduced = reduced;
    this.applyFlags();
    if (reduced) {
      this.inertia.cancel();
      this.gestures.cancel();
      this.morph?.cancel();
      this.morph = null;
    }
    this.orbit.pause(reduced || this.paused);
    this.scrambler.setInstant(reduced);
    this.refreshLife();
  }

  destroy(): void {
    window.clearTimeout(this.switchTimer);
    window.clearTimeout(this.deepTimer);
    window.clearInterval(this.mouthTimer);
    window.clearTimeout(this.pointerIdle);
    this.pointerStop?.();
    this.life.clear();
    this.stopLevel();
    this.blinker.run(false);
    this.scrambler.stop();
    this.orbit.stop();
    this.inertia.cancel();
    this.sway.cancel();
    this.gestures.cancel();
    this.cleanups.forEach((fn) => fn());
    this.root.remove();
  }

  /** Après une pause ou un changement de réglage : relance ou coupe la vie. */
  private refreshLife(): void {
    this.life.clear();
    this.endScene();
    this.scrambler.express(null);
    this.awake = false;
    this.glance = null;
    window.clearTimeout(this.deepTimer);
    if (this.live && this.shown && RESTING.has(this.shown) && !this.deep) this.deepTimer = window.setTimeout(() => this.deepSleep(), DEEP_SLEEP_MS);
    if (!this.live) {
      this.scrambler.finish();
      // Gestes, rebonds de lettres, gerbe… : les animations ponctuelles se
      // terminent tout de suite (les boucles, elles, sont figées par la scène).
      for (const anim of this.root.getAnimations({ subtree: true }))
        if (!(anim instanceof CSSAnimation) && !(anim instanceof CSSTransition) && anim.effect?.getTiming().iterations !== Infinity) anim.finish();
      this.gestures.cancel();
      this.inertia.cancel();
      this.sway.cancel();
      this.morph = null;
      this.stopLevel();
      this.pointer = null;
      window.clearTimeout(this.pointerIdle);
      this.pointerStop?.();
      this.pointerStop = null;
      this.lean.style.transform = '';
    }
    this.aim();
    this.syncTimers();
    this.startLife();
    this.scrambler.resync();
  }

  /**
   * Minuteries de l'état courant, seulement fenêtre visible et animations
   * permises : bouche qui parle (Répond) et clignements.
   */
  private syncTimers(): void {
    const live = this.live;
    const state = this.shown;
    this.blinker.run(live && state !== null && (BLINKS.has(state) || (state === 'idle' && this.awake)));
    const talk = live && state === 'answer' && (this.speech.length > 0 || this.data.answer === undefined);
    if (talk && !this.mouthTimer) {
      this.mouthTimer = window.setInterval(() => this.speak(), MOUTH_MS);
    } else if (!talk && this.mouthTimer) {
      window.clearInterval(this.mouthTimer);
      this.mouthTimer = 0;
    }
  }

  /** Une image de bouche, d'après les caractères qui viennent d'arriver. */
  private speak(): void {
    if (this.data.answer === undefined) {
      // Pas de texte : la bouche parle au rythme du prototype.
      this.mouthStep = (this.mouthStep + 1) % MOUTH.length;
      this.mouth.textContent = MOUTH[this.mouthStep]!;
      return;
    }
    if (!this.speech) {
      this.mouth.textContent = '·';
      this.syncTimers();
      return;
    }
    const chunk = this.speech.slice(0, 3);
    this.speech = this.speech.slice(3);
    this.mouthStep += 1;
    this.mouth.textContent = /[.,;:!?…]/.test(chunk) ? '-' : VOWELS.test(chunk) ? (this.mouthStep % 2 ? 'O' : 'o') : 'o';
  }

  private blink(closed: boolean): void {
    this.scrambler.blink(closed);
    if (closed && this.live) {
      // Les paupières s'écrasent (en plus de la boucle CSS de l'œil).
      for (const g of this.eyes) g.animate([{ transform: 'scaleY(1)' }, { transform: 'scaleY(.25)', offset: 0.45 }, { transform: 'scaleY(1)' }], { duration: 160, composite: 'add' });
    }
  }

  /** Expression passagère des yeux. */
  private flash(eyes: Eyes, ms: number): void {
    this.scrambler.express(eyes);
    this.life.after(ms, () => this.scrambler.express(null));
  }

  /** Direction du regard : curseur, coup d'œil, ou celle de l'état. */
  private aim(): void {
    const state = this.state ?? 'idle';
    let [x, y] = LOOKS[state].gaze;
    // La bulle est loin à gauche : un gain plus doux, pour que les yeux suivent le curseur d'un bout à l'autre.
    if (this.target && this.live) [x, y] = this.towardPointer(this.target, 0.55);
    else if (this.pointer && this.live) [x, y] = this.towardPointer();
    else if (this.glance) [x, y] = this.glance;
    const t = `translate(${x.toFixed(1)}px,${y.toFixed(1)}px)`;
    if (this.gaze.style.transform !== t) this.gaze.style.transform = t;
  }

  private towardPointer(p: { x: number; y: number } = this.pointer!, gain = 1.4): [number, number] {
    const r = this.pill.getBoundingClientRect();
    const dx = (p.x - (r.left + r.width / 2)) / Math.max(1, r.width);
    const dy = (p.y - (r.top + r.height / 2)) / Math.max(1, r.height);
    const clamp = (v: number) => Math.max(-1, Math.min(1, v));
    return [clamp(dx * gain) * 24, clamp(dy * gain) * 18];
  }

  /** Le regard et le corps suivent le curseur quand il passe sur la fenêtre. */
  private listenPointer(): void {
    const doc = document;
    const onMove = (e: PointerEvent): void => {
      if (!this.live) return;
      this.pointer = { x: e.clientX, y: e.clientY };
      window.clearTimeout(this.pointerIdle);
      this.pointerIdle = window.setTimeout(() => this.losePointer(), 2600);
      if (this.state === 'idle' && !this.awake) this.wake();
      if (!this.pointerStop) this.pointerStop = onFrame(() => this.followPointer());
    };
    const onLeave = (e: MouseEvent): void => {
      if (!e.relatedTarget) this.losePointer();
    };
    const onEnterPill = (): void => {
      const now = performance.now();
      if (!this.live || this.gestures.busy || now - this.lastBoop < 3000) return;
      this.lastBoop = now;
      this.gestures.play('boop');
      // Content qu'on vienne le voir : les yeux sourient un instant.
      if (this.state && HAPPY_ON_HOVER.has(this.state)) this.flash(['^', '^'], 520);
    };
    const onPress = (e: PointerEvent): void => {
      if (e.button === 0) this.pinch();
    };
    doc.addEventListener('pointermove', onMove);
    doc.addEventListener('mouseout', onLeave);
    this.pill.addEventListener('pointerenter', onEnterPill);
    this.pill.addEventListener('pointerdown', onPress);
    this.cleanups.push(() => {
      doc.removeEventListener('pointermove', onMove);
      doc.removeEventListener('mouseout', onLeave);
    });
  }

  private followPointer(): false {
    this.pointerStop = null;
    if (!this.pointer || !this.live) return false;
    this.aim();
    const [x] = this.towardPointer();
    const t = `rotate(${(x / 24) * 4}deg) translateX(${(x / 24) * 5}px)`;
    if (this.lean.style.transform !== t) this.lean.style.transform = t;
    return false;
  }

  private losePointer(): void {
    window.clearTimeout(this.pointerIdle);
    if (!this.pointer) return;
    this.pointer = null;
    this.lean.style.transform = '';
    this.aim();
    if (this.awake) this.life.after(900, () => this.sleepAgain());
  }

  /** Veille : le survol réveille Kobi, qui ouvre les yeux et suit le curseur. */
  private wake(): void {
    this.awake = true;
    this.life.clear();
    this.endScene();
    window.clearTimeout(this.deepTimer);
    if (this.deep) {
      this.deep = false;
      this.applyFlags();
    }
    this.scrambler.express(['o', 'o']);
    if (!this.gestures.busy) this.gestures.play('boop');
    this.syncTimers();
  }

  private sleepAgain(): void {
    if (this.state !== 'idle' || this.pointer || this.target) return;
    this.awake = false;
    this.scrambler.express(['–', 'o']);
    this.life.after(260, () => {
      this.scrambler.express(null);
      this.syncTimers();
      this.startLife();
      window.clearTimeout(this.deepTimer);
      this.deepTimer = window.setTimeout(() => this.deepSleep(), DEEP_SLEEP_MS);
    });
  }

  /** Vie de l'état : petits gestes espacés, jamais pendant une pause. */
  private startLife(): void {
    if (!this.live || this.shown !== this.state) return;
    // Dans tous les états, une lettre du mot sautille ou se brouille de temps en temps.
    this.every(this.deep ? 5000 : 2200, this.deep ? 9000 : 5200, () => this.letterLife());
    switch (this.state) {
      case 'idle':
        if (!this.awake) this.life.after(this.deep ? rand(12_000, 22_000) : rand(4_500, 9_000), () => this.idleEvent());
        break;
      case 'listen':
        this.every(1400, 3000, () => this.lookAround(6, 5, rand(400, 800)));
        break;
      case 'think':
        this.every(3500, 6000, () => this.lookAround(14, 8, rand(500, 900)));
        break;
      case 'write':
        this.every(1800, 3600, () => this.lookAround(8, 6, rand(300, 600)));
        break;
      case 'answer':
        this.every(2500, 4500, () => this.lookAround(10, 6, rand(500, 900)));
        break;
      case 'suggest':
        this.every(2600, 4200, () => !this.gestures.busy && this.gestures.play('nudge'));
        break;
      case 'done':
        this.every(3800, 6500, () => !this.gestures.busy && this.gestures.play('wiggle'));
        break;
      case 'error':
        this.every(1800, 3200, () => this.flash(['>', '<'], 420));
        break;
    }
  }

  /** Une lettre au hasard sautille (le plus souvent) ou se brouille. */
  private letterLife(): void {
    const letters = [...this.word.children].filter((c) => (c.textContent ?? '').trim()) as HTMLElement[];
    if (letters.length === 0 || this.scrambler.scrambling) return;
    const i = Math.floor(Math.random() * letters.length);
    const roll = Math.random();
    if (roll < 0.2 && letters.length > 1) {
      // Ola : toutes les lettres sautent l'une après l'autre.
      letters.forEach((letter, j) =>
        letter.animate(
          [
            { transform: 'none' },
            { transform: 'translateY(-18px) scale(.94,1.1)', offset: 0.4 },
            { transform: 'translateY(2px) scale(1.06,.95)', offset: 0.75 },
            { transform: 'none' },
          ],
          { duration: 520, delay: j * 80, easing: 'ease-in-out' },
        ),
      );
      return;
    }
    if (roll < 0.45) {
      this.scrambler.glitch([...this.word.children].indexOf(letters[i]!));
      return;
    }
    letters[i]!.animate(
      [
        { transform: 'none', easing: 'cubic-bezier(.45,0,.75,.4)' },
        { transform: 'translateY(6px) scale(1.15,.8)', offset: 0.15, easing: 'cubic-bezier(.2,.8,.3,1)' },
        { transform: 'translateY(-26px) scale(.9,1.15) rotate(-8deg)', offset: 0.45, easing: 'cubic-bezier(.45,0,.75,.4)' },
        { transform: 'translateY(0) scale(1.12,.86)', offset: 0.7 },
        { transform: 'none' },
      ],
      { duration: 620 },
    );
  }

  /** Répète `fn` à intervalle aléatoire tant que l'état dure. */
  private every(min: number, max: number, fn: () => void): void {
    this.life.after(rand(min, max), () => {
      fn();
      this.every(min, max, fn);
    });
  }

  /** Coup d'œil autour de la direction de l'état, puis retour. */
  private lookAround(rx: number, ry: number, hold: number): void {
    if (this.pointer) return;
    const [x, y] = LOOKS[this.state ?? 'idle'].gaze;
    this.glance = [x + rand(-rx, rx), y + rand(-ry, ry)];
    this.aim();
    this.life.after(hold, () => {
      this.glance = null;
      this.aim();
    });
  }

  /** Petite animation rare de la veille, tirée au hasard (jamais deux fois la même). */
  private idleEvent(): void {
    const pool = (Object.entries(IDLE_EVENTS) as Array<[IdleEvent, number]>).filter(([name]) => name !== this.lastIdleEvent);
    let pick = Math.random() * pool.reduce((s, [, w]) => s + w, 0);
    const chosen = pool.find(([, w]) => (pick -= w) < 0) ?? pool[0]!;
    this.runIdle(chosen[0]);
  }

  /** Noms des scènes de la veille (tests, vidéos). */
  idleNames(): IdleEvent[] {
    return Object.keys(IDLE_EVENTS) as IdleEvent[];
  }

  /** Joue une scène de la veille (tests, vidéos) ; sans effet hors veille ou sans animations. */
  playIdle(name: IdleEvent): boolean {
    if (!this.live || this.shown !== 'idle' || this.state !== 'idle') return false;
    this.life.clear();
    this.runIdle(name);
    return true;
  }

  private runIdle(name: IdleEvent): void {
    this.lastIdleEvent = name;
    this.root.dataset.idle = name;
    this.setLively('scene', true);
    const duration = this.idleScene(name);
    this.life.after(duration, () => {
      delete this.root.dataset.idle;
      this.setLively('scene', false);
      this.startLife();
    });
  }

  private setLively(reason: string, on: boolean): void {
    const before = this.lively.size > 0;
    if (on) this.lively.add(reason);
    else this.lively.delete(reason);
    const after = this.lively.size > 0;
    if (after !== before) this.onLivelyChange?.(after);
  }

  /** Scène interrompue (changement d'état, réveil, pause) : regard, yeux et bouche reprennent leur place. */
  private endScene(): void {
    // Symboles d'une scène ou d'un geste coupés en route : jamais laissés sur Kobi.
    this.burst.querySelectorAll('.fly').forEach((n) => n.remove());
    if (!this.root.dataset.idle) return;
    delete this.root.dataset.idle;
    this.setLively('scene', false);
    this.glance = null;
    this.scrambler.express(null);
    if (this.shown) this.mouth.textContent = LOOKS[this.shown].mouth;
    this.aim();
  }

  /** Lance la scène et renvoie sa durée (ms). */
  private idleScene(name: IdleEvent): number {
    switch (name) {
      case 'glitch':
        this.scrambler.glitch(Math.floor(Math.random() * 3));
        return 400;
      case 'peek':
        return this.peek();
      case 'doze':
        this.gestures.play('doze');
        this.life.after(1500, () => this.flash(['o', 'o'], 520));
        return 2300;
      case 'yawn':
        return this.yawn();
      case 'shiver':
        this.gestures.play('shiver');
        return 600;
      case 'look':
        return this.lookAroundAwake();
      case 'whistle':
        return this.whistle();
      case 'juggle':
        return this.juggle();
      case 'tap':
        return this.tapFoot();
      case 'sneeze':
        return this.sneeze();
      case 'hiccup':
        return this.hiccup();
      case 'wink':
        this.scrambler.express(['o', '–']);
        this.mouth.textContent = 'u';
        this.gestures.play('nod');
        this.life.after(650, () => {
          this.scrambler.express(null);
          this.mouth.textContent = LOOKS.idle.mouth;
        });
        return 900;
      case 'snore':
        return this.snore();
      case 'hum':
        this.gestures.play('sway');
        this.scrambler.express(['^', '^']);
        this.mouth.textContent = '~';
        this.life.after(500, () => this.fly('~', { x: 110, y: -30, dx: 70, dy: -90, size: 44, duration: 1300 }));
        this.life.after(1300, () => this.fly('~', { x: 110, y: -30, dx: 90, dy: -110, size: 38, duration: 1300 }));
        this.life.after(2150, () => {
          this.scrambler.express(null);
          this.mouth.textContent = LOOKS.idle.mouth;
        });
        return 2400;
      case 'reach':
        this.gestures.play('reach');
        this.scrambler.express(['>', '<']);
        this.life.after(1550, () => this.scrambler.express(null));
        return 1900;
      case 'roll':
        return this.rollEyes();
      case 'twirl':
        this.gestures.play('twirl');
        this.scrambler.express(['*', '*']);
        this.life.after(560, () => this.confetti(['+', '·', '*', '·', '+'], 0.45));
        this.life.after(1050, () => this.scrambler.express(null));
        return 1300;
      case 'dream':
        return this.dream();
    }
  }

  /** Un œil s'entrouvre, regarde à gauche puis à droite, se referme. */
  private peek(): number {
    this.scrambler.express(['–', 'o']);
    this.glance = [-12, 2];
    this.aim();
    this.life.after(650, () => {
      this.glance = [12, 2];
      this.aim();
    });
    this.life.after(1300, () => {
      this.glance = null;
      this.aim();
      this.scrambler.express(null);
    });
    return 1500;
  }

  private yawn(): number {
    this.gestures.play('stretch');
    const frames: Array<[number, string, Eyes | null]> = [
      [200, 'o', ['>', '<']],
      [420, 'O', ['>', '<']],
      [1250, 'o', ['>', '<']],
      [1500, '', null],
    ];
    for (const [t, mouth, eyes] of frames)
      this.life.after(t, () => {
        this.mouth.textContent = mouth;
        this.scrambler.express(eyes);
      });
    return 1800;
  }

  /** Rare : Kobi ouvre les yeux, fait le tour de la pièce du regard, se rendort. */
  private lookAroundAwake(): number {
    this.scrambler.express(['o', 'o']);
    const path: Array<[number, readonly [number, number] | null]> = [
      [150, [-16, 0]],
      [700, [16, -4]],
      [1250, [0, -12]],
      [1750, null],
    ];
    for (const [t, g] of path)
      this.life.after(t, () => {
        this.glance = g;
        this.aim();
      });
    this.life.after(1900, () => this.blink(true));
    this.life.after(2020, () => {
      this.blink(false);
      this.scrambler.express(null);
    });
    return 2300;
  }

  /** Sifflote : yeux rieurs, bouche en « o », des notes s'envolent. */
  private whistle(): number {
    this.gestures.play('sway');
    this.scrambler.express(['^', '^']);
    this.mouth.textContent = 'o';
    ['♪', '♫', '♪', '♫'].forEach((n, i) =>
      this.life.after(150 + i * 480, () => this.fly(n, { x: 100, y: -40, dx: rand(40, 90), dy: -rand(100, 150), size: rand(40, 52), duration: 1500, rotate: rand(-30, 30) })),
    );
    this.life.after(2100, () => {
      this.mouth.textContent = LOOKS.idle.mouth;
      this.scrambler.express(null);
    });
    return 2400;
  }

  /** Jongle avec les lettres de son nom ; les yeux suivent les lettres en l'air. */
  private juggle(): number {
    const lap = 900;
    const laps = 3;
    this.scrambler.express(['o', 'o']);
    this.glance = [0, -14];
    this.aim();
    ['k', 'o', 'b'].forEach((letter, i) => {
      const span = this.flyer(letter, 40);
      const anim = span.animate(
        [
          { transform: 'translate(-60px,-50px) rotate(0deg)', easing: 'cubic-bezier(.2,.6,.4,1)' },
          { transform: 'translate(0px,-190px) rotate(180deg)', offset: 0.5, easing: 'cubic-bezier(.6,0,.8,.4)' },
          { transform: 'translate(60px,-50px) rotate(330deg)', offset: 0.85 },
          { transform: 'translate(-60px,-50px) rotate(360deg)' },
        ],
        { duration: lap, delay: (i * lap) / 3, iterations: laps, fill: 'backwards' },
      );
      anim.onfinish = () => span.remove();
      anim.oncancel = () => span.remove();
    });
    this.life.after(lap * laps + lap * 0.7, () => {
      this.glance = null;
      this.aim();
      this.scrambler.express(null);
      this.gestures.play('nod');
    });
    return lap * laps + lap;
  }

  /** Tape du pied en regardant ailleurs ; chaque coup soulève un peu de poussière. */
  private tapFoot(): number {
    this.gestures.play('footTap');
    this.scrambler.express(['o', 'o']);
    this.glance = [16, 4];
    this.aim();
    for (const t of [190, 590, 990, 1390]) this.life.after(t, () => this.fly('·', { x: 110, y: 92, dx: rand(10, 30), dy: -rand(10, 24), size: 30, duration: 500 }));
    this.life.after(1650, () => {
      this.glance = null;
      this.aim();
      this.scrambler.express(null);
    });
    return 1900;
  }

  /** A… a… atchoum ! */
  private sneeze(): number {
    this.gestures.play('sneeze');
    this.scrambler.express(['>', '<']);
    this.life.after(250, () => (this.mouth.textContent = 'o'));
    this.life.after(650, () => (this.mouth.textContent = 'O'));
    this.life.after(930, () => {
      this.mouth.textContent = '>';
      this.scrambler.express(['x', 'x']);
      for (let i = 0; i < 7; i++) {
        const side = i % 2 ? 1 : -1;
        this.fly(i % 3 ? '*' : '·', { x: side * 60, y: 30, dx: side * rand(130, 220), dy: rand(-60, 50), size: rand(30, 44), duration: rand(650, 950), rotate: rand(-90, 90) });
      }
      this.scrambler.glitch(Math.floor(Math.random() * 3));
    });
    this.life.after(1450, () => {
      this.mouth.textContent = LOOKS.idle.mouth;
      this.scrambler.express(null);
    });
    return 1700;
  }

  /** Deux hoquets. */
  private hiccup(): number {
    this.gestures.play('hiccup');
    for (const t of [60, 860]) {
      this.life.after(t, () => {
        this.flash(['O', 'O'], 280);
        this.fly('hic', { x: 110, y: -70, dx: 50, dy: -60, size: 34, duration: 850 });
      });
    }
    return 1600;
  }

  /** S'endort profondément (une bulle enfle au nez), la bulle éclate : réveil en sursaut. */
  private snore(): number {
    this.gestures.play('sink');
    const bubble = this.flyer('o', 34);
    const grow = bubble.animate(
      [
        { transform: 'translate(40px,-10px) scale(.3)', opacity: 0.6 },
        { transform: 'translate(40px,-10px) scale(1.1)', opacity: 1, offset: 0.3 },
        { transform: 'translate(40px,-10px) scale(.7)', offset: 0.5 },
        { transform: 'translate(40px,-10px) scale(1.6)', offset: 0.8 },
        { transform: 'translate(40px,-10px) scale(1.2)', offset: 0.9 },
        { transform: 'translate(40px,-10px) scale(2)' },
      ],
      // Sans `fill` : la bulle se retire d'elle-même à la fin de son
      // animation, même si la scène est coupée (KOB-171 : restée posée sur
      // l'œil droit « o », on lisait « 8 »).
      { duration: 2400 },
    );
    grow.onfinish = () => bubble.remove();
    grow.oncancel = () => bubble.remove();
    this.life.after(2400, () => {
      bubble.remove();
      for (let i = 0; i < 5; i++) this.fly('·', { x: 40, y: -10, dx: rand(-50, 50), dy: rand(-50, 20), size: 26, duration: 450 });
      this.gestures.play('startle');
      this.scrambler.express(['O', 'O']);
      this.glance = [-14, 0];
      this.aim();
    });
    this.life.after(2900, () => {
      this.glance = [14, 0];
      this.aim();
    });
    this.life.after(3400, () => {
      this.glance = null;
      this.aim();
      this.scrambler.express(['–', 'o']);
    });
    this.life.after(3700, () => this.scrambler.express(null));
    return 3900;
  }

  /** Les yeux font un tour complet. */
  private rollEyes(): number {
    this.scrambler.express(['@', '@']);
    const steps = 8;
    for (let i = 0; i <= steps; i++) {
      const a = (i / steps) * Math.PI * 2 - Math.PI / 2;
      this.life.after(i * 140, () => {
        this.glance = i === steps ? null : [Math.cos(a) * 16, Math.sin(a) * 12];
        this.aim();
      });
    }
    this.life.after(steps * 140 + 200, () => this.scrambler.express(null));
    return steps * 140 + 400;
  }

  /** Rêve : des bulles de pensée montent, jusqu'à une question qui s'évapore. */
  private dream(): number {
    const path: Array<[number, string, number, number]> = [
      [0, 'o', 60, -70],
      [350, 'o', 80, -110],
      [700, 'O', 100, -150],
      [1100, '?', 110, -190],
    ];
    for (const [t, g, x, y] of path)
      this.life.after(t, () => this.fly(g, { x, y, dx: rand(-8, 8), dy: -30, size: g === '?' ? 46 : 22 + t / 60, duration: 1500 }));
    this.gestures.play('doze');
    return 2700;
  }

  /** Clic sur Kobi : il se fait pincer (écrasé), puis sursaute de surprise. */
  pinch(): void {
    if (!this.live) return;
    if (this.state === 'idle' && !this.awake) this.wake();
    this.lastBoop = performance.now();
    this.gestures.play('pinch');
    this.scrambler.express(['>', '<']);
    this.root.dataset.pinched = '1';
    this.life.after(240, () => {
      this.gestures.play('surprise');
      this.scrambler.express(['O', 'O']);
      this.fly('!', { x: 90, y: -70, dx: 30, dy: -60, size: 60, duration: 700, rotate: 15 });
    });
    this.life.after(1000, () => {
      delete this.root.dataset.pinched;
      this.scrambler.express(this.state === 'idle' && this.awake ? ['o', 'o'] : null);
    });
  }

  /** La fenêtre s'est déplacée de (dx, dy) px : Kobi ballotte, les yeux s'arrondissent. */
  jostle(dx: number, dy: number): void {
    if (!this.live || (dx === 0 && dy === 0)) return;
    this.setLively('sway', true);
    this.sway.push(dx, dy);
    if (Math.hypot(dx, dy) > 6) this.flash(['O', 'O'], 400);
  }

  /**
   * Déplacement de la fenêtre native, en px physiques : `onMoved` de la
   * fenêtre courante (l'événement `tauri://move` vise la fenêtre, une écoute
   * globale ne le reçoit pas), sinon l'événement lui-même (coque simulée des tests).
   */
  private listenWindow(): void {
    type Moved = (e: { payload: { x: number; y: number } }) => void;
    type Unlisten = Promise<() => void>;
    const api = (
      window as unknown as {
        __TAURI__?: {
          window?: { getCurrentWindow?: () => { onMoved?: (h: Moved) => Unlisten } };
          event?: { listen?: (event: string, h: Moved) => Unlisten };
        };
      }
    ).__TAURI__;
    if (!api) return;
    let last: { x: number; y: number } | null = null;
    const handler: Moved = (e) => {
      const p = e.payload;
      if (!p || typeof p.x !== 'number' || typeof p.y !== 'number') return;
      const ratio = window.devicePixelRatio || 1;
      if (last) this.jostle((p.x - last.x) / ratio, (p.y - last.y) / ratio);
      last = { x: p.x, y: p.y };
    };
    const current = api.window?.getCurrentWindow?.();
    const stop = current?.onMoved ? current.onMoved(handler) : api.event?.listen?.('tauri://move', handler);
    if (stop) this.cleanups.push(() => void stop.then((fn) => fn()));
  }

  /** Symbole posé au centre de la pilule (calque de la gerbe), à animer. */
  private flyer(text: string, size: number): HTMLElement {
    const span = el('span', 'fly', text);
    span.style.fontSize = `${size}px`;
    this.burst.append(span);
    return span;
  }

  /** Un symbole s'envole de (x, y) et s'efface. */
  private fly(text: string, f: Flight): void {
    if (!this.live) return;
    const span = this.flyer(text, f.size ?? 34);
    const r = f.rotate ?? 0;
    const anim = span.animate(
      [
        { transform: `translate(${f.x}px,${f.y}px) scale(.4) rotate(0deg)`, opacity: 0 },
        { transform: `translate(${f.x + f.dx * 0.3}px,${f.y + f.dy * 0.3}px) scale(1.1) rotate(${r * 0.4}deg)`, opacity: 1, offset: 0.25 },
        { transform: `translate(${f.x + f.dx}px,${f.y + f.dy}px) scale(.9) rotate(${r}deg)`, opacity: 0 },
      ],
      { duration: f.duration ?? 1000, easing: 'cubic-bezier(.2,.7,.4,1)' },
    );
    anim.onfinish = () => span.remove();
    anim.oncancel = () => span.remove();
  }

  /** Efface l'effet de l'état qui s'en va pendant l'anticipation. */
  private fadeOut(state: MascotState | null): void {
    const selector = state ? FX[state] : undefined;
    if (!selector || (state && FX[state] === FX[this.state!])) return;
    const node = this.body.querySelector<HTMLElement>(selector);
    if (node) this.fading.push(node.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 120, fill: 'forwards' }));
  }

  /** Fait surgir l'effet du nouvel état. */
  private popIn(state: MascotState): void {
    const selector = FX[state];
    const node = selector ? this.body.querySelector<HTMLElement>(selector) : null;
    if (!node) return;
    node.animate(
      [
        { opacity: 0, transform: 'scale(.3)' },
        { opacity: 1, transform: 'scale(1.15)', offset: 0.6 },
        { opacity: 1, transform: 'scale(1)' },
      ],
      { duration: 380, easing: 'cubic-bezier(.2,.8,.3,1)', composite: 'add' },
    );
  }

  /** Le mot s'écrase puis se redresse quand il change. */
  private squashWord(): void {
    this.word.animate(
      [
        { transform: 'none', easing: 'cubic-bezier(.45,0,.75,.4)' },
        { transform: 'translateY(16px) scale(1.2,.66)', offset: 0.22, easing: 'cubic-bezier(.2,.8,.3,1.2)' },
        { transform: 'translateY(-6px) scale(.94,1.1)', offset: 0.55 },
        { transform: 'none' },
      ],
      { duration: 560 },
    );
  }

  /** Étincelles de symboles à chaque transition. */
  private sparks(count: number): void {
    const noise = '#%&@*?!/<>{}=+$';
    this.confetti(Array.from({ length: count }, () => noise.charAt(Math.floor(Math.random() * noise.length))), 0.55);
  }

  /** Fait ! : une gerbe de symboles jaillit de la pilule. */
  private confetti(glyphs = ['+', '*', '·', '!', '^', '+', '*', 'o', '·', '+', '!', '*'], reach = 1): void {
    glyphs.forEach((g, i) => {
      const span = el('span', '', g);
      this.burst.append(span);
      const angle = -Math.PI * (0.08 + (0.84 * i) / (glyphs.length - 1)) + rand(-0.12, 0.12);
      const dist = rand(130, 210) * reach;
      const dx = Math.cos(angle) * dist;
      const dy = Math.sin(angle) * dist * 0.8;
      const anim = span.animate(
        [
          { transform: 'translate(0,0) scale(.2) rotate(0deg)', opacity: 1 },
          { transform: `translate(${dx * 0.8}px,${dy * 0.8}px) scale(1.1) rotate(${rand(-90, 90)}deg)`, opacity: 1, offset: 0.55 },
          { transform: `translate(${dx}px,${dy + 40}px) scale(.8) rotate(${rand(-180, 180)}deg)`, opacity: 0 },
        ],
        { duration: rand(900, 1200), delay: i * 14, easing: 'cubic-bezier(.15,.75,.35,1)', fill: 'backwards' },
      );
      anim.onfinish = () => span.remove();
      anim.oncancel = () => span.remove();
    });
  }

  /** Passe le repos en mode économe : ondulation du mot par paliers, un seul « z ». */
  private deepSleep(): void {
    if (!this.state || !RESTING.has(this.state) || this.awake) return;
    // Une scène de la veille se joue : le repos économe attend qu'elle finisse.
    if (this.root.dataset.idle) {
      this.deepTimer = window.setTimeout(() => this.deepSleep(), 1000);
      return;
    }
    // Les « z » changent d'animation : chacune reprend là où en était l'ancienne.
    const zs = [...this.root.querySelectorAll<HTMLElement>('.zz span')];
    const times = zs.map((z) => z.getAnimations()[0]?.currentTime ?? null);
    this.deep = true;
    this.applyFlags();
    zs.forEach((z, i) => {
      const anim = z.getAnimations()[0];
      if (anim && times[i] !== null) anim.currentTime = times[i]!;
    });
    // Le rythme de la vie ralentit.
    this.life.clear();
    this.startLife();
  }

  // ----- Écrire à Kobi (KOB-147) : la bulle de saisie fait vivre Kobi. -----

  /**
   * Kobi regarde un point de la fenêtre (le curseur de la bulle d'écriture),
   * avant le curseur de la souris ; `null` le relâche. En veille, il se réveille
   * pour lire, puis se rendort quand on le relâche.
   */
  lookAt(point: { x: number; y: number } | null): void {
    const had = this.target !== null;
    this.target = point;
    if (point && this.live && this.state === 'idle' && !this.awake) {
      this.wake();
      this.reading(true);
    }
    if (!point && had) {
      this.reading(false);
      if (this.awake && !this.pointer) this.life.after(900, () => this.sleepAgain());
    }
    this.aim();
  }

  /** En veille, Kobi lit ce qu'on lui écrit : plus de « zzz », il dit « je lis ». */
  private reading(on: boolean): void {
    if (this.root.classList.contains('reading') === on) return;
    this.root.classList.toggle('reading', on);
    if (this.state !== 'idle') return;
    if (on) this.scrambler.show('o', 'je lis');
    else this.scrambler.show(LOOKS.idle.eye, LOOKS.idle.word);
  }

  /** Une touche frappée dans la bulle : petit sursaut. */
  keyJolt(): void {
    if (!this.live) return;
    this.pill.animate([{ transform: 'none' }, { transform: 'translateY(-5px) scale(.97,1.04)', offset: 0.35 }, { transform: 'none' }], {
      duration: 190,
      easing: 'cubic-bezier(.2,.8,.3,1)',
      composite: 'add',
    });
  }

  /** On hésite au milieu d'une phrase : Kobi s'impatiente (regard en coin, il tapote). */
  impatient(): void {
    if (!this.live) return;
    this.scrambler.express(['¬', '¬']);
    if (!this.gestures.busy) this.gestures.play('tap');
    this.life.after(900, () => this.scrambler.express(this.awake ? ['o', 'o'] : null));
  }

  /**
   * Le texte envoyé arrive dans sa bouche : Kobi l'ouvre grand, avale au bout
   * de `gulpMs`, savoure. La scène passe ensuite en Réfléchit.
   */
  swallow(gulpMs: number): void {
    if (!this.live) return;
    this.mouth.textContent = 'O';
    this.scrambler.express(['O', 'O']);
    this.life.after(gulpMs, () => {
      this.mouth.textContent = 'o';
      this.gestures.play('gulp');
    });
    this.life.after(gulpMs + 160, () => {
      this.mouth.textContent = '';
      this.scrambler.express(['^', '^']);
    });
  }

  /** Bouche de Kobi (coordonnées de la fenêtre) : la bulle d'écriture en sort et y rentre. */
  mouthRect(): DOMRect {
    return this.mouth.getBoundingClientRect();
  }
}
