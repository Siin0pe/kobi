// Scène : héberge Kobi, ses bulles et cartes et la région lue par les
// lecteurs d'écran. Elle applique les réglages système (réduire les
// animations) et met tout en pause quand la fenêtre est masquée.

import { animationsReduitesSelonReglage, suivreAnimations } from './animations';
import { Bubbles, type ActionDecision } from './bubbles';
import { suivreCadence } from './cadence';
import { Metronome } from './econome';
import { Kobi } from './mascotte/kobi';
import { ConsentCard, RecIndicator, type ConsentEvent } from './meeting';
import {
  DOT_COLORS,
  type ConsentLang,
  type MeetingInfo,
  STATE_LABELS,
  STATE_SPOKEN,
  type MascotData,
  type MascotState,
} from './states';

/** Fiche de notes pendant un enregistrement, tant que la coque n'envoie pas la dernière note. */
export const NOTES_PLACEHOLDER = 'J’écoute et je prends des notes.';

/** Taille logique de la scène, en px ; elle est mise à l'échelle de la fenêtre. */
export const SCENE_WIDTH = 820;
export const SCENE_HEIGHT = 560;

export interface StageOptions {
  /**
   * Fond affiché derrière Kobi (captures, développement) : le vert de la
   * marque (`true`), ou un bureau sombre ou clair pour vérifier la lisibilité.
   * La fenêtre native reste transparente.
   */
  backdrop?: boolean | 'sombre' | 'clair';
  /** Force « réduire les animations » (tests) ; sinon suit le réglage « Animations » de Kobi. */
  reducedMotion?: boolean;
  /** Se cale sur l'élément parent (aperçu) au lieu de la fenêtre. */
  inFrame?: boolean;
  /** Annonce les changements d'état aux lecteurs d'écran (désactivé pour un aperçu). */
  announce?: boolean;
}

/** Un rectangle de la fenêtre, en px (vue agrandie, KOB-169). */
export interface Boite {
  x: number;
  y: number;
  largeur: number;
  hauteur: number;
}

export class Stage {
  readonly root: HTMLElement;
  private readonly scene: HTMLElement;
  private readonly live: HTMLElement;
  private readonly bubbles: Bubbles;
  private readonly dot: HTMLButtonElement;
  private readonly rec = new RecIndicator();
  private consent: ConsentCard | null = null;
  private meeting: MeetingInfo | null = null;
  /** Appelé quand l'utilisateur copie le message ou confirme avoir prévenu les participants. */
  onConsent: ((e: ConsentEvent) => void) | null = null;
  private compact = false;
  /** Appelé quand l'utilisateur déplie la pastille (la coque peut agrandir la fenêtre). */
  onCompactChange: ((compact: boolean) => void) | null = null;
  readonly kobi: Kobi;
  /** État affiché (Note au lieu de Veille pendant un enregistrement). */
  private state: MascotState = 'idle';
  private data: MascotData = {};
  /** État demandé par la session ou l'appelant, et ses données. */
  private requested: MascotState = 'idle';
  private requestedData: MascotData = {};
  private spoken: MascotState | null = null;
  private paused = false;
  private windowHidden = false;
  private reducedMotion: boolean;
  private readonly forcedMotion: boolean | undefined;
  private readonly inFrame: boolean;
  private readonly cleanups: Array<() => void> = [];
  /** Échelle de la scène hors agrandissement. */
  private fitted = 0;
  /** Échelle gardée pendant que la fenêtre change de taille. */
  private scaleFrozen = false;
  /** Animations arrêtées par la mise en pause, à relancer au retour. */
  private frozen: Animation[] = [];
  private readonly metronome: Metronome;
  /** Réponse interrompue par l'utilisateur (Échap). */
  private interrupted = false;
  /** Appelé à chaque état affiché (l'historique se referme, par exemple). */
  onStateChange: ((state: MascotState) => void) | null = null;
  /** Appelé à chaque rendu des bulles (état ou données) : réponse longue, KOB-163. */
  onRender: ((state: MascotState, data: Readonly<MascotData>) => void) | null = null;
  /** Fenêtre agrandie (vue de la conversation) : où reste la petite fenêtre de Kobi. */
  private placement: Boite | null = null;
  /** État ignoré quelque temps (voir `ignore`). */
  private ignored: { state: MascotState; until: number } | null = null;

  constructor(parent: HTMLElement, options: StageOptions = {}) {
    this.forcedMotion = options.reducedMotion;
    this.reducedMotion = options.reducedMotion ?? animationsReduitesSelonReglage();

    this.root = document.createElement('div');
    this.root.className = 'stage';
    this.root.classList.toggle('with-backdrop', Boolean(options.backdrop));
    if (typeof options.backdrop === 'string') this.root.dataset.backdrop = options.backdrop;

    this.scene = document.createElement('div');
    this.scene.className = 'scene';
    this.scene.setAttribute('role', 'img');

    this.bubbles = new Bubbles();

    this.live = document.createElement('div');
    this.live.className = 'sr-only';
    if (options.announce ?? true) {
      this.live.setAttribute('role', 'status');
      this.live.setAttribute('aria-live', 'polite');
    }

    // Mode discret : une pastille de 32 px colorée selon l'état ; un clic la déplie.
    this.dot = document.createElement('button');
    this.dot.type = 'button';
    this.dot.className = 'dot';
    this.dot.hidden = true;
    this.dot.addEventListener('click', () => this.setCompact(false));

    this.kobi = new Kobi(this.reducedMotion);
    this.scene.append(this.kobi.root, this.bubbles.root, this.rec.root);
    // Images par seconde réglées (KOB-200) : toute la fenêtre (Kobi, bulles,
    // REC, champ, vue agrandie) avance au rythme d'un seul métronome (econome.ts).
    this.metronome = new Metronome(this.root, (fps) => (this.root.dataset.fps = String(fps)));
    this.root.dataset.fps = '0';
    let deep = false;
    let economie = false;
    // Repos économe, seulement avec « Économie d'énergie » (désactivée par défaut).
    const repos = (): void => {
      const on = deep && economie;
      this.root.classList.toggle('econome', on);
      // Après le changement de style : les boucles du repos sont en place.
      if (on) requestAnimationFrame(() => this.root.classList.contains('econome') && this.metronome.setActive(true));
      else this.metronome.setActive(false);
    };
    this.kobi.onDeepChange = (d) => {
      deep = d;
      repos();
    };
    this.cleanups.push(
      suivreCadence((cadence, voulue, r) => {
        this.root.dataset.cadence = String(voulue);
        this.metronome.setCadence(cadence);
        economie = r.economie;
        repos();
      }),
    );
    // Scène de la veille ou ballottement pendant le repos : le geste se joue
    // à 60 images/s, les boucles de fond restent au rythme du métronome
    // (KOB-149, KOB-170).
    this.kobi.onLivelyChange = (lively) => {
      this.root.toggleAttribute('data-geste', lively);
      this.metronome.setLively(lively);
    };
    this.cleanups.push(() => this.metronome.destroy());
    this.root.append(this.scene, this.dot, this.live);
    parent.append(this.root);

    this.inFrame = options.inFrame ?? false;
    this.root.classList.toggle('in-frame', this.inFrame);
    this.fit();
    if (this.inFrame) {
      const observer = new ResizeObserver(() => this.fit());
      observer.observe(parent);
      this.cleanups.push(() => observer.disconnect());
    } else {
      const onResize = (): void => this.fit();
      window.addEventListener('resize', onResize);
      this.cleanups.push(() => window.removeEventListener('resize', onResize));
    }

    const onVisibility = (): void => this.applyPaused();
    document.addEventListener('visibilitychange', onVisibility);
    this.cleanups.push(() => document.removeEventListener('visibilitychange', onVisibility));

    // Réglage « Animations » de Kobi (toujours, comme Windows, réduites),
    // changé ici ou dans les Réglages, ou réglage de Windows en mode « comme Windows ».
    if (this.forcedMotion === undefined) this.cleanups.push(suivreAnimations((reduced) => this.setReducedMotion(reduced)));
    this.root.classList.toggle('reduced-motion', this.reducedMotion);
    this.bubbles.setReducedMotion(this.reducedMotion);
    this.rec.setReducedMotion(this.reducedMotion);
    this.setState('idle');
    this.applyPaused();
  }

  get currentState(): MascotState {
    return this.state;
  }

  /** Change d'état. Les données remplacent les précédentes. */
  setState(state: MascotState, data: MascotData = {}): void {
    if (this.ignored && this.ignored.state === state && performance.now() < this.ignored.until) return;
    if (state !== 'answer') this.interrupted = false;
    this.requested = state;
    this.requestedData = { ...data };
    this.show();
  }

  /**
   * Pendant un enregistrement (hors pause), le repos de Kobi est la prise de
   * notes : une demande de Veille affiche Note, avec la dernière note tapée
   * dans la fiche. Les autres états (Écoute, Réfléchit, Répond…) s'affichent
   * tels quels, puis Kobi retourne à ses notes.
   */
  private show(): void {
    const taking = this.requested === 'idle' && this.meeting !== null && !this.meeting.paused && !this.meeting.stopping;
    const state: MascotState = taking ? 'write' : this.requested;
    const data = taking ? { notes: this.meeting?.lastNote ?? NOTES_PLACEHOLDER } : this.requestedData;
    if (state === this.state && taking) {
      this.update(data);
      return;
    }
    this.state = state;
    this.data = { ...data };
    this.root.dataset.state = state;
    this.scene.setAttribute('aria-label', `Kobi : ${STATE_LABELS[state]}`);
    this.paintDot();
    this.kobi.setState(state, this.data);
    this.bubbles.render(state, this.data);
    this.onStateChange?.(state);
    this.onRender?.(state, this.data);
    if (state !== this.spoken) {
      this.spoken = state;
      this.live.textContent = STATE_SPOKEN[state];
    }
  }

  /**
   * Ignore les demandes de `state` pendant `ms` : un texte tapé est avalé par
   * Kobi, qui réfléchit déjà ; l'Écoute que la coque annonce ensuite ne
   * ressortirait pas la question (KOB-147).
   */
  ignore(state: MascotState, ms: number): void {
    this.ignored = { state, until: performance.now() + ms };
  }

  get isCompact(): boolean {
    return this.compact;
  }

  /**
   * Mode discret : la mascotte et les cartes laissent place à la pastille.
   * Kobi est mis en pause tant qu'il est caché (aucun coût CPU).
   */
  setCompact(compact: boolean): void {
    if (compact === this.compact) return;
    this.compact = compact;
    this.root.classList.toggle('compact', compact);
    this.scene.hidden = compact;
    this.dot.hidden = !compact;
    this.applyPaused();
    this.onCompactChange?.(compact);
  }

  get currentMeeting(): MeetingInfo | null {
    return this.meeting;
  }

  /**
   * Début de réunion : l'indicateur REC s'affiche jusqu'à `endMeeting`, et la
   * carte qui rappelle de prévenir les participants s'ouvre (sauf `consent: false`).
   */
  startMeeting(info: MeetingInfo, options: { consent?: boolean; lang?: ConsentLang } = {}): void {
    this.setMeeting(info);
    this.say(info.title ? `Réunion « ${info.title} » : transcription en local.` : 'Réunion : transcription en local.');
    if (options.consent ?? true) this.openConsent(options.lang ?? 'fr');
  }

  /** Met à jour la réunion (titre, pause). */
  updateMeeting(patch: Partial<MeetingInfo>): void {
    if (!this.meeting) return;
    const wasPaused = Boolean(this.meeting.paused);
    const wasStopping = Boolean(this.meeting.stopping);
    const notes = this.meeting.notes ?? 0;
    this.setMeeting({ ...this.meeting, ...patch });
    if ((this.meeting.notes ?? 0) > notes && !this.isPaused()) this.kobi.noteAdded();
    const stopping = Boolean(this.meeting.stopping);
    if (stopping !== wasStopping) this.say(stopping ? 'Enregistrement arrêté. Dis « reprends » pour continuer.' : 'Transcription reprise.');
    else if (Boolean(this.meeting.paused) !== wasPaused) this.say(this.meeting.paused ? 'Transcription en pause.' : 'Transcription reprise.');
  }

  endMeeting(): void {
    if (!this.meeting) return;
    this.closeConsent();
    this.setMeeting(null);
    this.say('Réunion terminée.');
  }

  private setMeeting(info: MeetingInfo | null): void {
    const wasTaking = this.state === 'write' && this.requested === 'idle';
    this.meeting = info;
    this.rec.set(info);
    this.kobi.setRecording(info === null ? null : { paused: Boolean(info.paused) });
    this.root.classList.toggle('in-meeting', info !== null);
    this.dot.classList.toggle('recording', info !== null && !info.paused);
    this.paintDot();
    const taking = this.requested === 'idle' && info !== null && !info.paused;
    if (taking || wasTaking) this.show();
  }

  private openConsent(lang: ConsentLang): void {
    this.closeConsent();
    const card = new ConsentCard();
    card.setLang(lang);
    card.onEvent = (e) => {
      this.onConsent?.(e);
      if (e.action === 'done') this.closeConsent();
    };
    this.consent = card;
    this.scene.classList.add('consent-open');
    this.scene.append(card.root);
    // Une demande qui attend l'utilisateur : la mascotte repliée se déplie.
    if (this.compact) this.setCompact(false);
    card.root.querySelector<HTMLButtonElement>('.btn-primary')?.focus({ preventScroll: true });
  }

  private closeConsent(): void {
    this.consent?.root.remove();
    this.consent = null;
    this.scene.classList.remove('consent-open');
  }

  private say(text: string): void {
    this.live.textContent = text;
  }

  /** Phrase lue par les lecteurs d'écran (compte rendu prêt…). */
  announce(text: string): void {
    this.say(text);
  }

  /** Met à jour les données sans changer d'état (texte en flux, sources). */
  update(data: MascotData): void {
    // Réponse interrompue (Échap) : la suite du flux ne s'affiche plus.
    if (this.interrupted && data.answer !== undefined) {
      const { answer: _ignored, ...rest } = data;
      data = rest;
    }
    this.data = { ...this.data, ...data };
    this.kobi.setData(this.data);
    this.bubbles.render(this.state, this.data);
    this.onRender?.(this.state, this.data);
  }

  /**
   * Échap ou « stop » : la réponse s'arrête net, le texte déjà affiché reste ;
   * la suite du flux est ignorée jusqu'au prochain changement d'état.
   */
  interrupt(): void {
    if (this.state !== 'answer') return;
    this.interrupted = true;
    this.bubbles.interrupt();
    // Le texte affiché devient la réponse : un nouveau rendu ne relance pas la frappe.
    this.data = { ...this.data, answer: this.root.querySelector('[data-card=answer] .text')?.textContent ?? '' };
    this.kobi.setData(this.data);
  }

  /** « Lire en grand » sur la bulle de réponse (vue agrandie, KOB-169). */
  onReadAll(handler: () => void): void {
    this.bubbles.onReadAll = handler;
  }

  get agrandie(): Boite | null {
    return this.placement;
  }

  /**
   * Fenêtre agrandie pour la vue de la conversation (KOB-169) : la scène
   * reste exactement où était la petite fenêtre (`kobi`), à la même échelle ;
   * Kobi ne bouge pas à l'écran. `null` : retour à la taille habituelle.
   */
  setPlacement(kobi: Boite | null): void {
    this.placement = kobi;
    this.root.classList.toggle('agrandie', kobi !== null);
    if (kobi) {
      this.root.style.setProperty('--kobi-x', `${kobi.x + kobi.largeur / 2}px`);
      this.root.style.setProperty('--kobi-y', `${kobi.y + kobi.hauteur / 2}px`);
    }
    this.fit();
  }

  /** Garde l'échelle pendant que la fenêtre change de taille (agrandissement). */
  freezeScale(frozen: boolean): void {
    this.scaleFrozen = frozen;
    if (!frozen) this.fit();
  }

  /** Ajoute un élément à la scène (champ de saisie, historique, notifications). */
  mount(node: HTMLElement): void {
    this.scene.append(node);
  }

  /** Volume du micro, de 0 à 1 : Kobi l'écoute en Écoute. */
  setLevel(level: number): void {
    if (!this.isPaused()) this.kobi.setLevel(level);
  }

  /** Appelé quand l'utilisateur confirme ou refuse une carte d'action. */
  onAction(handler: (decision: ActionDecision) => void): void {
    this.bubbles.onAction = handler;
  }

  setReducedMotion(reduced: boolean): void {
    this.reducedMotion = reduced;
    this.root.classList.toggle('reduced-motion', reduced);
    this.bubbles.setReducedMotion(reduced);
    this.rec.setReducedMotion(reduced);
    this.kobi.setReducedMotion(reduced);
  }

  destroy(): void {
    this.rec.destroy();
    this.kobi.destroy();
    this.cleanups.forEach((fn) => fn());
    this.root.remove();
  }

  /** Pause : fenêtre masquée, ou mascotte repliée en pastille. */
  private isPaused(): boolean {
    return document.visibilityState === 'hidden' || this.compact;
  }

  private paintDot(): void {
    const color = DOT_COLORS[this.state];
    this.dot.style.setProperty('--dot', color);
    this.dot.dataset.state = this.state;
    const rec = this.meeting ? (this.meeting.paused ? ', réunion en pause' : ', réunion transcrite') : '';
    this.dot.setAttribute('aria-label', `Kobi : ${STATE_LABELS[this.state]}${rec}. Déplier la mascotte`);
    this.dot.title = `Kobi : ${STATE_LABELS[this.state]}`;
  }

  private applyPaused(): void {
    const paused = this.isPaused();
    const hidden = document.visibilityState === 'hidden';
    if (paused === this.paused && hidden === this.windowHidden) return;
    this.paused = paused;
    this.windowHidden = hidden;
    // `paused` arrête la scène (mascotte et cartes) ; `window-hidden` arrête
    // tout, pastille comprise.
    this.root.classList.toggle('paused', paused);
    this.root.classList.toggle('window-hidden', hidden);
    // Kobi d'abord : sa mise en pause recentre le regard (transition CSS) ;
    // le gel ci-dessous la saisit avec le reste.
    this.kobi.setPaused(paused);
    this.frozen.forEach((a) => {
      if (a.playState === 'paused') a.play();
    });
    this.frozen = [];
    const scope = hidden ? this.root : paused ? this.scene : null;
    if (scope) {
      // Animations CSS, transitions en vol, animations Web de Kobi.
      this.frozen = scope.getAnimations({ subtree: true }).filter((a) => a.playState === 'running');
      this.frozen.forEach((a) => a.pause());
    }
    this.metronome.setSuspended(paused || hidden);
    this.bubbles.setPaused(paused);
    this.rec.setPaused(hidden);
  }

  private fit(): void {
    const box = this.inFrame ? this.root.parentElement!.getBoundingClientRect() : { width: window.innerWidth, height: window.innerHeight };
    // Agrandie : l'échelle d'avant reste, la place gagnée sert à la vue.
    const keep = (this.placement || this.scaleFrozen) && this.fitted;
    const scale = keep ? this.fitted : Math.min(box.width / SCENE_WIDTH, box.height / SCENE_HEIGHT);
    if (!keep) this.fitted = scale;
    this.root.style.setProperty('--scale', String(scale));
  }
}
