// Réunion : indicateur REC visible pendant toute la réunion et carte qui
// rappelle de prévenir les participants, avec un message prêt à coller (FR et
// EN), aux couleurs de Kobi (RGPD : les
// participants doivent savoir qu'ils sont transcrits).

import { CONSENT_MESSAGES, type ConsentLang, type MeetingInfo } from './states';

export interface ConsentEvent {
  /** `copy` : message copié ; `done` : l'utilisateur a prévenu les participants. */
  action: 'copy' | 'done';
  lang: ConsentLang;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Durée écoulée au format 12:34 ou 1:02:03. */
export function elapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // Repli sans API presse-papiers (permissions, ancien moteur).
    const area = el('textarea');
    area.value = text;
    area.style.cssText = 'position:fixed;opacity:0';
    document.body.append(area);
    area.select();
    const ok = document.execCommand('copy');
    area.remove();
    return ok;
  }
}

/**
 * Indicateur « ● REC · titre · 12:34 · 3 notes », collé à la mascotte, mis à
 * jour une fois par seconde, aux chiffres qui roulent. Une note ajoutée fait
 * rebondir le compteur ; le point rouge pulse.
 */
export class RecIndicator {
  readonly root: HTMLElement;
  private readonly label: HTMLElement;
  private readonly time: HTMLElement;
  private readonly notes: HTMLElement;
  /** « son de l'ordi » : la piste « eux » est captée (KOB-164). */
  private readonly eux: HTMLElement;
  private reduced = false;
  private info: MeetingInfo | null = null;
  private timer = 0;
  private paused = false;
  /** Sortie en cours (« réunion terminée · 7 min ») après la fin (KOB-166). */
  private ending = 0;
  private endAnim: Animation | null = null;

  constructor() {
    this.root = el('div', 'rec');
    this.root.hidden = true;
    // Pas de région « live » : la durée change chaque seconde et serait lue en
    // boucle. La scène annonce le début, la pause et la fin de la réunion.
    this.time = el('span', 'rec-time');
    this.time.setAttribute('aria-hidden', 'true');
    this.notes = el('span', 'rec-notes');
    this.notes.hidden = true;
    this.eux = el('span', 'rec-eux', 'son de l’ordi');
    this.eux.title = 'Le son de l’ordinateur (la voix des autres en visio) est transcrit aussi.';
    this.eux.hidden = true;
    this.root.append(el('span', 'rec-dot'), (this.label = el('span', 'rec-label')), this.time, this.notes, this.eux);
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
  }

  get active(): boolean {
    return this.info !== null;
  }

  set(info: MeetingInfo | null): void {
    const before = this.info?.notes ?? 0;
    const ended = this.info !== null && info === null ? this.info : null;
    this.info = info;
    this.stopEnding();
    if (ended) {
      this.end(ended);
      return;
    }
    this.root.hidden = info === null;
    this.root.classList.toggle('is-paused', Boolean(info?.paused));
    if (info) {
      const word = info.paused ? 'PAUSE' : 'REC';
      this.label.textContent = info.title ? `${word} · ${info.title}` : word;
      this.root.setAttribute('aria-label', info.paused ? 'Transcription en pause' : 'Réunion transcrite en local');
      const n = info.notes ?? 0;
      this.notes.hidden = n === 0;
      this.digits(this.notes, String(n), ` note${n > 1 ? 's' : ''}`);
      this.eux.hidden = !info.computerAudio || Boolean(info.paused);
      if (n > before && !this.reduced && !this.paused) this.bump(n - before);
    }
    this.tick();
  }

  /**
   * Fin de la réunion (KOB-166) : l'enregistrement est fini à l'instant. Le
   * point s'éteint, l'étiquette devient grise « réunion terminée · 7 min »,
   * puis elle s'efface. Rien n'y ressemble plus à un enregistrement en cours.
   */
  private end(info: MeetingInfo): void {
    window.clearTimeout(this.timer);
    this.timer = 0;
    const minutes = Math.round((Date.now() - info.startedAt) / 60_000);
    this.root.classList.remove('is-paused');
    this.root.classList.add('is-ended');
    this.root.setAttribute('aria-label', 'Réunion terminée');
    this.label.textContent = 'réunion terminée';
    this.time.textContent = minutes < 1 ? 'moins d’une minute' : `${minutes} min`;
    this.notes.hidden = true;
    this.root.hidden = false;
    const hide = (): void => {
      this.ending = 0;
      this.endAnim = null;
      this.root.hidden = true;
      this.root.classList.remove('is-ended');
    };
    if (this.reduced || this.paused) {
      this.ending = window.setTimeout(hide, 1600);
      return;
    }
    this.root.querySelector('.rec-dot')!.animate(
      [
        { transform: 'scale(1)', opacity: 1 },
        { transform: 'scale(1.6)', opacity: 1, offset: 0.35 },
        { transform: 'scale(.6)', opacity: 0.6 },
      ],
      { duration: 420, easing: 'cubic-bezier(.2,.8,.3,1)', fill: 'forwards' },
    );
    this.endAnim = this.root.animate(
      [
        { opacity: 1, translate: '0 0', scale: '1' },
        { opacity: 1, translate: '0 0', scale: '1', offset: 0.8 },
        { opacity: 0, translate: '0 -10px', scale: '.85' },
      ],
      { duration: 2000, easing: 'ease-in' },
    );
    this.endAnim.onfinish = hide;
  }

  private stopEnding(): void {
    window.clearTimeout(this.ending);
    this.ending = 0;
    this.endAnim?.cancel();
    this.endAnim = null;
    for (const a of this.root.querySelector('.rec-dot')?.getAnimations() ?? []) if (!(a instanceof CSSAnimation)) a.cancel();
    this.root.classList.remove('is-ended');
  }

  /** Note ajoutée : le compteur rebondit et un « +1 » s'envole. */
  private bump(added: number): void {
    this.notes.animate(
      [
        { transform: 'scale(1)' },
        { transform: 'scale(1.45) rotate(-6deg)', offset: 0.3 },
        { transform: 'scale(.92)', offset: 0.6 },
        { transform: 'scale(1)' },
      ],
      { duration: 520, easing: 'cubic-bezier(.2,.8,.3,1)' },
    );
    const plus = el('span', 'rec-plus', `+${added}`);
    plus.setAttribute('aria-hidden', 'true');
    this.root.append(plus);
    const anim = plus.animate(
      [
        { transform: 'translateY(0) scale(.6)', opacity: 0 },
        { transform: 'translateY(-14px) scale(1.15)', opacity: 1, offset: 0.3 },
        { transform: 'translateY(-34px) scale(1)', opacity: 0 },
      ],
      { duration: 900, easing: 'cubic-bezier(.2,.7,.3,1)' },
    );
    anim.onfinish = () => plus.remove();
    anim.oncancel = () => plus.remove();
  }

  /**
   * Chiffres qui roulent : un élément par caractère ; celui qui change arrive
   * par le haut comme sur un compteur mécanique (animation Web, tenue par le
   * métronome au repos). `suffixe` suit les chiffres (« notes »). Le texte de
   * `node` reste lisible tel quel (« 12:34 », « 3 notes »).
   */
  private digits(node: HTMLElement, chiffres: string, suffixe = ''): void {
    const chars = Array.from(chiffres);
    let spans = [...node.querySelectorAll<HTMLElement>(':scope > .chiffre')];
    if (spans.length !== chars.length) {
      node.replaceChildren(...chars.map((c) => el('span', 'chiffre', c)));
      spans = [...node.querySelectorAll<HTMLElement>(':scope > .chiffre')];
    } else {
      chars.forEach((c, i) => {
        const span = spans[i]!;
        if (span.textContent === c) return;
        span.textContent = c;
        if (this.reduced || this.paused) return;
        span.animate(
          [
            { transform: 'translateY(-85%)', opacity: 0 },
            { transform: 'translateY(12%)', opacity: 1, offset: 0.7 },
            { transform: 'none', opacity: 1 },
          ],
          { duration: 340, easing: 'cubic-bezier(.2,.8,.3,1)' },
        );
      });
    }
    const tail = node.lastChild;
    if (suffixe) {
      if (tail?.nodeType === Node.TEXT_NODE) {
        if (tail.textContent !== suffixe) tail.textContent = suffixe;
      } else node.append(document.createTextNode(suffixe));
    } else if (tail?.nodeType === Node.TEXT_NODE) tail.remove();
  }

  /** Fenêtre masquée : plus de minuterie ; la durée est recalculée au retour. */
  setPaused(paused: boolean): void {
    this.paused = paused;
    this.tick();
  }

  destroy(): void {
    window.clearTimeout(this.timer);
    this.stopEnding();
  }

  private tick(): void {
    window.clearTimeout(this.timer);
    this.timer = 0;
    if (!this.info) return;
    const ms = Date.now() - this.info.startedAt;
    this.digits(this.time, elapsed(ms));
    if (this.paused) return;
    // Se cale sur la seconde suivante.
    this.timer = window.setTimeout(() => this.tick(), 1000 - (ms % 1000) + 5);
  }
}

/** Carte du démarrage : prévenir les participants, message à copier en FR ou EN. */
export class ConsentCard {
  readonly root: HTMLElement;
  onEvent: ((e: ConsentEvent) => void) | null = null;
  private lang: ConsentLang = 'fr';
  private readonly message: HTMLElement;
  private readonly copyButton: HTMLButtonElement;
  private readonly langButtons: Record<ConsentLang, HTMLButtonElement>;

  constructor() {
    this.root = el('div', 'card card-consent');
    this.root.dataset.card = 'consent';
    this.root.setAttribute('role', 'dialog');
    this.root.setAttribute('aria-labelledby', 'consent-titre');
    const title = el('div', 'title', 'Prévenez les participants');
    title.id = 'consent-titre';
    const langs = el('div', 'langs');
    langs.setAttribute('role', 'group');
    langs.setAttribute('aria-label', 'Langue du message');
    this.langButtons = { fr: el('button', 'lang', 'FR'), en: el('button', 'lang', 'EN') };
    for (const l of ['fr', 'en'] as const) {
      const b = this.langButtons[l];
      b.type = 'button';
      b.addEventListener('click', () => this.setLang(l));
      langs.append(b);
    }
    this.message = el('blockquote', 'message');
    this.copyButton = el('button', 'btn btn-primary', 'Copier le message');
    this.copyButton.type = 'button';
    this.copyButton.addEventListener('click', () => void this.copy());
    const done = el('button', 'btn', 'C’est fait');
    done.type = 'button';
    done.addEventListener('click', () => this.onEvent?.({ action: 'done', lang: this.lang }));
    const buttons = el('div', 'buttons');
    buttons.append(this.copyButton, done);
    this.root.append(
      el('div', 'kicker', 'Réunion · transcription locale'),
      title,
      el('p', 'intro', 'Kobi transcrit cette réunion. Collez ce message dans le chat de l’appel :'),
      langs,
      this.message,
      buttons,
    );
    this.setLang('fr');
  }

  setLang(lang: ConsentLang): void {
    this.lang = lang;
    this.message.textContent = CONSENT_MESSAGES[lang];
    this.message.lang = lang;
    for (const l of ['fr', 'en'] as const) this.langButtons[l].setAttribute('aria-pressed', String(l === lang));
    this.copyButton.textContent = 'Copier le message';
  }

  private async copy(): Promise<void> {
    const ok = await copy(CONSENT_MESSAGES[this.lang]);
    this.copyButton.textContent = ok ? 'Copié ✓' : 'Copie impossible : sélectionnez le texte';
    if (ok) this.onEvent?.({ action: 'copy', lang: this.lang });
  }
}
