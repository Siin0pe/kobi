// Écrire à Kobi (KOB-147) : une bulle de dialogue qui sort de sa bouche, à la
// place de l'ancienne barre permanente. Au repos, rien d'autre que Kobi.
//
// La bulle s'ouvre d'un clic sur Kobi, à la première touche tapée quand la
// fenêtre a le focus, par l'entrée « Écrire à Kobi » du menu (événement DOM
// `kobi:ecrire`) ou par le raccourci global (événement `ecrire` de la coque).
// Elle grandit depuis la bouche de Kobi avec un rebond ; Entrée l'envoie
// (les lettres sont aspirées, Kobi avale, puis réfléchit) ; Échap, ou une
// bulle vide pendant 8 s, la dégonfle et la fait rentrer dans Kobi. Pendant
// une conversation (KOB-133), elle reste ouverte entre deux échanges et se
// range seule à la fin.
//
// Le champ est un vrai `<textarea>` (copier-coller, sélection, saisie des
// accents), au texte transparent : un miroir dessiné dessous affiche les
// mêmes lettres, qui arrivent avec le brouillage de caractères de Kobi. Les
// deux ont la même police à chasse fixe (Recursive MONO) : un symbole de
// bruit a la largeur de la lettre, le miroir ne bouge pas d'un pixel. Le
// miroir donne aussi la position du curseur, que les yeux de Kobi suivent.

import { surLangue, t } from './i18n';
import { NOISE } from './mascotte/glyphs';
import { tauri } from './native';
import { SCENE_WIDTH, type Stage } from './stage';
import type { MascotState } from './states';
import { onFrame } from './ticker';

/** Bulle vide sans rien faire : elle rentre dans Kobi. */
export const IDLE_CLOSE_MS = 8_000;
/** Silence au milieu d'une phrase avant que Kobi s'impatiente, puis entre deux impatiences. */
const HESITATE_MS = 2_600;
const HESITATE_AGAIN_MS = 4_000;
const HESITATE_MAX = 3;
/** Brouillage d'une lettre tapée : un pas toutes les 60 ms, posée au 3e pas. */
const STEP_MS = 60;
const SCRAMBLE_STEPS = 3;
/** Au-delà (collage d'un long texte), les lettres se posent sans brouillage. */
const SCRAMBLE_MAX = 160;
/** Lettres aspirées à l'envoi : au plus tant, et toute l'aspiration en moins de ~0,5 s. */
const FLY_MAX = 140;
const FLY_MS = 360;
const FLY_SPREAD_MS = 200;
/** États où Kobi est pris par un échange : la bulle ne se range pas seule. */
const BUSY: ReadonlySet<MascotState> = new Set(['listen', 'think', 'answer', 'suggest']);

const LETTER_POP: Keyframe[] = [
  { transform: 'translateY(.35em) scale(.6,1.3)', easing: 'cubic-bezier(.2,.8,.3,1.2)' },
  { offset: 0.45, transform: 'translateY(-.25em) scale(1.12,.92)', easing: 'cubic-bezier(.45,0,.35,1)' },
  { offset: 0.75, transform: 'translateY(.05em) scale(.97,1.03)' },
  { transform: 'none' },
];

interface Lettre {
  c: string;
  span: HTMLElement;
  /** Instant où la lettre se pose (brouillage), 0 si posée. */
  settle: number;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function noise(seed: number): string {
  return NOISE.charAt(((seed % NOISE.length) + NOISE.length) % NOISE.length);
}

export class Ecrire {
  readonly form: HTMLFormElement;
  readonly field: HTMLTextAreaElement;
  private readonly mirror: HTMLElement;
  private readonly end: HTMLElement;
  private readonly flight: HTMLElement;
  /** Texte envoyé (Entrée), après l'avoir avalé. */
  onSend: ((text: string) => void) | null = null;
  /** Flèche haut dans la bulle vide : l'historique (KOB-133). */
  onHistory: (() => void) | null = null;
  /** Échap laissé aux autres (historique ouvert, réponse à interrompre). */
  yields: (() => boolean) | null = null;
  private letters: Lettre[] = [];
  private shown = false;
  private anim: Animation | null = null;
  private scrambleStop: (() => void) | null = null;
  private idleTimer = 0;
  private hesitateTimer = 0;
  private hesitations = 0;
  private sendTimer = 0;
  private session: string | undefined;

  constructor(private readonly stage: Stage) {
    this.form = el('form', 'ecrire');
    this.form.hidden = true;
    this.form.dataset.etat = 'rangee';
    const zone = el('div', 'ecrire-zone');
    this.mirror = el('div', 'ecrire-miroir');
    this.mirror.setAttribute('aria-hidden', 'true');
    this.end = el('span', 'ecrire-fin', '​');
    this.mirror.append(this.end);
    this.field = el('textarea', 'ecrire-champ');
    this.field.name = 'texte';
    this.field.rows = 1;
    this.field.autocomplete = 'off';
    this.field.spellcheck = false;
    this.textes();
    // Fixés au montage : ils suivent la langue à chaud.
    surLangue(() => this.textes());
    zone.append(this.mirror, this.field);
    const enter = el('span', 'ecrire-entree', '↵');
    enter.setAttribute('aria-hidden', 'true');
    this.form.append(zone, enter);
    this.flight = el('div', 'ecrire-vol');
    this.flight.setAttribute('aria-hidden', 'true');
    stage.mount(this.form);
    stage.mount(this.flight);

    this.form.addEventListener('submit', (e) => {
      e.preventDefault();
      this.send();
    });
    this.field.addEventListener('input', () => this.onInput());
    this.field.addEventListener('scroll', () => (this.mirror.scrollTop = this.field.scrollTop));
    this.field.addEventListener('keydown', (e) => this.onKey(e));
    document.addEventListener('selectionchange', () => {
      if (document.activeElement === this.field) this.look();
    });

    // Un clic sur Kobi ouvre la bulle (le glisser déplace la fenêtre, native.ts).
    stage.kobi.root.addEventListener('click', (e) => {
      if ((e.target as Element | null)?.closest('.body, .word')) this.open();
    });
    // Taper au clavier n'importe où : la bulle s'ouvre et reçoit la touche.
    document.addEventListener('keydown', (e) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || e.key.length !== 1 || e.isComposing) return;
      const target = e.target as Element | null;
      if (target?.closest('input, textarea, select, [contenteditable], [role=menu], [role=menuitem]')) return;
      // Espace sur un bouton ou « N étapes » : il les actionne (KOB-213).
      if (e.key === ' ' && target?.closest('button, summary')) return;
      // La touche qui ouvre la bulle y est écrite par nous : le navigateur ne
      // la redirige pas toujours vers un champ qui vient d'apparaître et de
      // prendre le focus dans la même frappe (WebView2), elle serait avalée.
      e.preventDefault();
      this.open();
      this.insert(e.key);
    });
    window.addEventListener('kobi:ecrire', () => this.open());
    // Menu du clic droit (KOB-139) : entrée `ecrire`.
    window.addEventListener('kobi:menu', (e) => (e as CustomEvent<string>).detail === 'ecrire' && this.open());
    void tauri()?.event.listen('ecrire', () => this.open());

    const previous = stage.onStateChange;
    stage.onStateChange = (state) => {
      previous?.(state);
      this.rearm();
    };
    // Fin de la conversation (la session quitte « assistant ») : la bulle vide se range.
    new MutationObserver(() => {
      const was = this.session;
      this.session = stage.root.dataset.session;
      if (was === 'assistant' && this.session !== 'assistant' && this.field.value.trim() === '') this.close();
      else this.rearm();
    }).observe(stage.root, { attributes: true, attributeFilter: ['data-session'] });
  }

  private textes(): void {
    this.form.setAttribute('aria-label', t('kobi.ecrire.aria'));
    this.field.placeholder = t('kobi.ecrire.placeholder');
    this.field.setAttribute('aria-label', t('kobi.ecrire.aria'));
  }

  get isOpen(): boolean {
    return this.shown;
  }

  /** Ouvre la bulle (ou y remet le focus) : elle sort de la bouche de Kobi. */
  open(): void {
    if (this.stage.isCompact) this.stage.setCompact(false);
    if (this.shown) {
      this.field.focus({ preventScroll: true });
      return;
    }
    this.shown = true;
    this.form.hidden = false;
    this.form.dataset.etat = 'ouverte';
    this.stage.root.classList.add('ecrire-open');
    this.field.focus({ preventScroll: true });
    this.grow();
    this.rearm();
  }

  /** Range la bulle : elle se dégonfle et rentre dans Kobi. */
  close(): void {
    if (!this.shown) return;
    this.shown = false;
    window.clearTimeout(this.idleTimer);
    window.clearTimeout(this.hesitateTimer);
    this.form.dataset.etat = 'fermeture';
    this.stage.root.classList.remove('ecrire-open');
    if (document.activeElement === this.field) this.field.blur();
    this.stage.kobi.lookAt(null);
    const done = (): void => {
      if (this.shown) return;
      this.form.hidden = true;
      this.form.dataset.etat = 'rangee';
      this.field.value = '';
      this.sync();
    };
    this.play(this.reduced ? this.fade(false) : this.deflate(), done);
  }

  private get reduced(): boolean {
    return this.stage.root.classList.contains('reduced-motion');
  }

  private onKey(e: KeyboardEvent): void {
    if (e.isComposing) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      this.send();
    } else if (e.key === 'Escape') {
      if (this.yields?.()) return;
      e.preventDefault();
      e.stopPropagation();
      this.close();
    } else if (e.key === 'ArrowUp' && this.field.value === '') {
      e.preventDefault();
      this.onHistory?.();
    }
  }

  /** Écrit `text` au curseur, comme une frappe (miroir, sursaut de Kobi). */
  private insert(text: string): void {
    const f = this.field;
    f.setRangeText(text, f.selectionStart, f.selectionEnd, 'end');
    this.onInput();
  }

  private onInput(): void {
    const before = this.letters.length;
    this.sync();
    if (this.letters.length !== before) this.stage.kobi.keyJolt();
    this.hesitations = 0;
    this.look();
    this.rearm();
  }

  /** Entrée : les lettres sont aspirées dans la bouche de Kobi, qui avale, puis réfléchit. */
  private send(): void {
    const text = this.field.value.trim();
    if (!text) return;
    const flown = this.reduced ? 0 : this.fly();
    this.field.value = '';
    this.sync();
    this.stage.kobi.lookAt(null);
    this.form.dataset.etat = 'envoi';
    window.clearTimeout(this.sendTimer);
    const deliver = (): void => {
      this.sendTimer = 0;
      if (this.form.dataset.etat === 'envoi') this.form.dataset.etat = this.shown ? 'ouverte' : 'rangee';
      // Kobi réfléchit déjà : l'Écoute que la coque annonce pour un texte tapé
      // ne ressort pas la question.
      this.stage.setState('think');
      this.stage.ignore('listen', 1_500);
      this.onSend?.(text);
      this.look();
      this.rearm();
    };
    if (flown === 0) {
      deliver();
      return;
    }
    this.stage.kobi.swallow(flown - 60);
    this.sendTimer = window.setTimeout(deliver, flown + 80);
  }

  /** Lance les lettres vers la bouche ; renvoie la durée de l'aspiration (ms). */
  private fly(): number {
    const scene = this.sceneBox();
    const mouth = this.mouthPoint(scene);
    const letters = this.letters.filter((l) => l.c.trim() !== '').slice(0, FLY_MAX);
    if (letters.length === 0) return 0;
    const font = getComputedStyle(this.mirror);
    const spread = letters.length > 1 ? Math.min(14, FLY_SPREAD_MS / (letters.length - 1)) : 0;
    letters.forEach((l, i) => {
      const r = l.span.getBoundingClientRect();
      const x = (r.left - scene.left) / scene.scale;
      const y = (r.top - scene.top) / scene.scale;
      const g = el('span', 'ecrire-vole', l.c);
      g.style.left = `${x}px`;
      g.style.top = `${y}px`;
      g.style.font = font.font;
      g.style.fontVariationSettings = font.fontVariationSettings;
      this.flight.append(g);
      const dx = mouth.x - x;
      const dy = mouth.y - y;
      // Aspirées : elles accélèrent, tournent sur elles-mêmes et rapetissent en s'engouffrant.
      const anim = g.animate(
        [
          { transform: 'translate(0,0) scale(1)', opacity: 1 },
          { transform: `translate(${dx * 0.18}px,${dy * 0.18 - 14}px) scale(1.15) rotate(${(i % 2 ? 1 : -1) * 12}deg)`, opacity: 1, offset: 0.3 },
          { transform: `translate(${dx}px,${dy}px) scale(.15) rotate(${(i % 2 ? 1 : -1) * 220}deg)`, opacity: 0.2 },
        ],
        { duration: FLY_MS, delay: i * spread, easing: 'cubic-bezier(.55,0,.9,.45)', fill: 'backwards' },
      );
      anim.onfinish = () => g.remove();
      anim.oncancel = () => g.remove();
    });
    return Math.round(spread * (letters.length - 1) + FLY_MS);
  }

  /** La bulle grandit depuis la bouche de Kobi, avec un rebond. */
  private grow(): void {
    // Les yeux se posent sur le curseur une fois la bulle à sa place.
    const settled = (): void => this.look();
    if (this.reduced) {
      this.play(this.fade(true), settled);
      return;
    }
    const [dx, dy] = this.toMouth();
    this.play(
      this.form.animate(
        [
          { transform: `translate(${dx}px,${dy}px) scale(.05)`, opacity: 0, easing: 'cubic-bezier(.2,.8,.3,1)' },
          { transform: 'translate(0,0) scale(1.08,1.12)', opacity: 1, offset: 0.55, easing: 'cubic-bezier(.45,0,.35,1)' },
          { transform: 'scale(.96,.94)', offset: 0.78 },
          { transform: 'scale(1.01)', offset: 0.9 },
          { transform: 'none', opacity: 1 },
        ],
        { duration: 520 },
      ),
      settled,
    );
  }

  /** Elle se dégonfle (petite inspiration, puis aspirée vers la bouche). */
  private deflate(): Animation {
    const [dx, dy] = this.toMouth();
    return this.form.animate(
      [
        { transform: 'none', opacity: 1, easing: 'cubic-bezier(.45,0,.35,1)' },
        { transform: 'scale(1.05,1.08)', opacity: 1, offset: 0.25, easing: 'cubic-bezier(.6,0,.9,.5)' },
        { transform: `translate(${dx}px,${dy}px) scale(.05)`, opacity: 0 },
      ],
      { duration: 380, fill: 'forwards' },
    );
  }

  /** Réduire les animations : fondu court. */
  private fade(visible: boolean): Animation {
    return this.form.animate([{ opacity: visible ? 0 : 1 }, { opacity: visible ? 1 : 0 }], { duration: 140, fill: visible ? 'none' : 'forwards' });
  }

  private play(anim: Animation, then?: () => void): void {
    this.anim?.cancel();
    this.anim = anim;
    anim.onfinish = () => {
      if (this.anim !== anim) return;
      this.anim = null;
      then?.();
      anim.cancel();
    };
  }

  /** Décalage (repère de la bulle) de la pointe de la bulle à la bouche de Kobi. */
  private toMouth(): [number, number] {
    const scene = this.sceneBox();
    const mouth = this.mouthPoint(scene);
    const style = getComputedStyle(this.form);
    const [ox, oy] = style.transformOrigin.split(' ').map(parseFloat) as [number, number];
    return [mouth.x - (this.form.offsetLeft + ox), mouth.y - (this.form.offsetTop + oy)];
  }

  private sceneBox(): { left: number; top: number; scale: number } {
    const r = this.form.parentElement!.getBoundingClientRect();
    return { left: r.left, top: r.top, scale: r.width / SCENE_WIDTH || 1 };
  }

  private mouthPoint(scene: { left: number; top: number; scale: number }): { x: number; y: number } {
    const m = this.stage.kobi.mouthRect();
    return { x: (m.left + m.width / 2 - scene.left) / scene.scale, y: (m.top + m.height / 2 - scene.top) / scene.scale };
  }

  /** Les yeux de Kobi suivent le curseur de la bulle. */
  private look(): void {
    if (!this.shown || document.activeElement !== this.field) return;
    const caret = this.field.selectionEnd;
    let units = 0;
    let node = this.end;
    for (const l of this.letters) {
      if (units >= caret) {
        node = l.span;
        break;
      }
      units += l.c.length;
    }
    const r = node.getBoundingClientRect();
    this.stage.kobi.lookAt({ x: r.left, y: r.top + r.height / 2 });
  }

  /** Bulle vide et Kobi libre : elle se range au bout de 8 s. On hésite : Kobi s'impatiente. */
  private rearm(): void {
    window.clearTimeout(this.idleTimer);
    window.clearTimeout(this.hesitateTimer);
    if (!this.shown) return;
    const busy = BUSY.has(this.stage.currentState) || this.stage.root.dataset.session === 'assistant' || this.sendTimer !== 0;
    if (this.field.value.trim() === '') {
      if (!busy) this.idleTimer = window.setTimeout(() => this.close(), IDLE_CLOSE_MS);
      return;
    }
    if (this.hesitations < HESITATE_MAX) this.hesitateTimer = window.setTimeout(() => this.hesitate(), this.hesitations ? HESITATE_AGAIN_MS : HESITATE_MS);
  }

  private hesitate(): void {
    this.hesitations += 1;
    this.stage.kobi.impatient();
    this.rearm();
  }

  /** Recopie le champ dans le miroir : seules les lettres changées sont touchées. */
  private sync(): void {
    const next = Array.from(this.field.value);
    const old = this.letters;
    let p = 0;
    while (p < old.length && p < next.length && old[p]!.c === next[p]) p++;
    let s = 0;
    while (s < old.length - p && s < next.length - p && old[old.length - 1 - s]!.c === next[next.length - 1 - s]) s++;
    const removed = old.splice(p, old.length - p - s);
    for (const l of removed) l.span.remove();
    const added = next.slice(p, next.length - s);
    const before = old[p]?.span ?? this.end;
    const now = performance.now();
    const scramble = !this.reduced && added.length <= SCRAMBLE_MAX;
    const fresh = added.map((c, j): Lettre => {
      const span = el('span', 'ecrire-c', c);
      const blank = c.trim() === '';
      const settle = scramble && !blank ? now + STEP_MS * (SCRAMBLE_STEPS + Math.min(j, 40) * 0.5) : 0;
      if (settle) {
        span.classList.add('nouv');
        span.textContent = noise(j * 7 + 3);
      }
      this.mirror.insertBefore(span, before);
      return { c, span, settle };
    });
    old.splice(p, 0, ...fresh);
    this.mirror.scrollTop = this.field.scrollTop;
    this.form.classList.toggle('has-text', next.some((c) => c.trim() !== ''));
    if (fresh.some((l) => l.settle) && !this.scrambleStop) this.scrambleStop = onFrame((t) => this.scrambleTick(t));
  }

  /** Les lettres tapées passent par des symboles, puis se posent en rebondissant. */
  private scrambleTick(now: number): boolean {
    let pending = false;
    const step = Math.floor(now / STEP_MS);
    this.letters.forEach((l, i) => {
      if (!l.settle) return;
      if (now < l.settle) {
        pending = true;
        const n = noise(i * 7 + step * 3);
        if (l.span.textContent !== n) l.span.textContent = n;
        return;
      }
      l.settle = 0;
      l.span.textContent = l.c;
      const pop = l.span.animate(LETTER_POP, { duration: 340 });
      const settled = (): void => l.span.classList.remove('nouv');
      pop.onfinish = settled;
      pop.oncancel = settled;
    });
    if (!pending) this.scrambleStop = null;
    return pending;
  }
}
