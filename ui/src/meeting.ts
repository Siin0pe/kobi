// Réunion : indicateur REC visible pendant toute la réunion et carte qui
// rappelle de prévenir les participants, avec un message prêt à coller (FR et
// EN), aux couleurs de Kobi (RGPD : les
// participants doivent savoir qu'ils sont transcrits).

import { CONSENT_MESSAGES, type ConsentLang, type MeetingInfo } from './states';
import { surLangue, t } from './i18n';
import { touches } from './touches';

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
  /** Retard de la transcription (KOB-250) : un ordre dit maintenant attendra. */
  private readonly retard: HTMLElement;
  /** « Annuler » après la fin (KOB-245) : la même réunion reprend. */
  private readonly undo: HTMLButtonElement;
  /** Fin encore annulable : l'étiquette reste, le bouton aussi. */
  private undoable = false;
  /** Clic sur « Annuler ». */
  onUndo: (() => void) | null = null;
  private reduced = false;
  private info: MeetingInfo | null = null;
  private timer = 0;
  private paused = false;
  /** Sortie en cours (« réunion terminée · 7 min ») après la fin (KOB-166). */
  private ending = 0;
  private endAnim: Animation | null = null;
  /** Réunion qui vient de finir (étiquette « réunion terminée »), pour la retraduire. */
  private ended: MeetingInfo | null = null;
  private readonly offLangue: () => void;

  constructor() {
    this.root = el('div', 'rec');
    this.root.hidden = true;
    // Pas de région « live » : la durée change chaque seconde et serait lue en
    // boucle. La scène annonce le début, la pause et la fin de la réunion.
    this.time = el('span', 'rec-time');
    this.time.setAttribute('aria-hidden', 'true');
    this.notes = el('span', 'rec-notes');
    this.notes.hidden = true;
    this.eux = el('span', 'rec-eux');
    this.eux.hidden = true;
    this.retard = el('span', 'rec-retard');
    this.retard.hidden = true;
    this.undo = el('button', 'rec-undo');
    this.undo.type = 'button';
    this.undo.hidden = true;
    this.undo.addEventListener('click', () => this.onUndo?.());
    this.root.append(el('span', 'rec-dot'), (this.label = el('span', 'rec-label')), this.time, this.notes, this.eux, this.retard, this.undo);
    this.fixes();
    // L'étiquette reste affichée : elle suit la langue à chaud.
    this.offLangue = surLangue(() => this.retraduire());
  }

  /** Textes qui ne dépendent pas de la réunion. */
  private fixes(): void {
    this.eux.textContent = t('kobi.meeting.sonOrdi');
    this.eux.title = t('kobi.meeting.sonOrdiAide');
    this.undo.textContent = t('kobi.meeting.annuler');
    this.undo.title = t('kobi.meeting.annulerAide');
  }

  /** Changement de langue : l'étiquette affichée se réécrit. */
  private retraduire(): void {
    this.fixes();
    if (this.info) this.textes(this.info);
    else if (this.ended && !this.root.hidden) this.textesFin(this.ended);
  }

  /** Étiquette, nom accessible et compteur de notes d'une réunion en cours. */
  private textes(info: MeetingInfo): void {
    const word = info.paused ? t('kobi.meeting.pause') : t('kobi.meeting.rec');
    this.label.textContent = info.title ? t('kobi.meeting.etiquette', { mot: word, titre: info.title }) : word;
    this.root.setAttribute('aria-label', info.paused ? t('kobi.meeting.ariaPause') : t('kobi.meeting.ariaRec'));
    const n = info.notes ?? 0;
    this.digits(this.notes, String(n), t('kobi.meeting.notes', { n }));
    this.montrerRetard(info.late);
  }

  /** « réunion terminée · 7 min », ou « réunion terminée · Annuler » tant que la fin s'annule. */
  private textesFin(info: MeetingInfo): void {
    const minutes = Math.round((Date.now() - info.startedAt) / 60_000);
    this.label.textContent = t('kobi.meeting.terminee');
    this.time.textContent = minutes < 1 ? t('kobi.meeting.moinsDUneMinute') : t('kobi.meeting.minutes', { n: minutes });
    this.root.setAttribute('aria-label', this.undoable ? t('kobi.meeting.ariaTermineeAnnuler') : t('kobi.meeting.ariaTerminee'));
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
  }

  get active(): boolean {
    return this.info !== null;
  }

  /** `undoable` : la fin s'annule encore (KOB-245), « Annuler » s'affiche. */
  set(info: MeetingInfo | null, undoable = false): void {
    const before = this.info?.notes ?? 0;
    const ended = this.info !== null && info === null ? this.info : null;
    this.info = info;
    this.stopEnding();
    if (ended) {
      this.end(ended, undoable);
      return;
    }
    this.root.hidden = info === null;
    this.root.classList.toggle('is-paused', Boolean(info?.paused));
    if (info) {
      this.ended = null;
      const n = info.notes ?? 0;
      this.notes.hidden = n === 0;
      this.textes(info);
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
  private end(info: MeetingInfo, undoable: boolean): void {
    window.clearTimeout(this.timer);
    this.timer = 0;
    this.ended = info;
    this.root.classList.remove('is-paused');
    this.root.classList.add('is-ended');
    this.textesFin(info);
    this.notes.hidden = true;
    this.root.hidden = false;
    if (undoable) {
      // « Réunion terminée · Annuler » (KOB-245) : l'étiquette attend la fin
      // du délai, puis s'efface comme d'habitude.
      this.undoable = true;
      this.undo.hidden = false;
      // « réunion terminée · Annuler » : la durée attend l'effacement.
      this.time.hidden = true;
      this.root.setAttribute('aria-label', t('kobi.meeting.ariaTermineeAnnuler'));
      return;
    }
    this.fade();
  }

  /** « retard 2 min » : la transcription suit la parole avec retard ; le raccourci arrête tout de suite. */
  private montrerRetard(late: MeetingInfo['late']): void {
    this.retard.hidden = !late;
    if (!late) {
      this.retard.removeAttribute('title');
      return;
    }
    const duree = late.seconds < 60 ? t('carnet.format.secondes', { s: late.seconds }) : t('carnet.format.minutes', { min: Math.round(late.seconds / 60) });
    const arret = touches(late.shortcut).join('+');
    this.retard.textContent = t('kobi.meeting.retard', { duree });
    this.retard.title = t('kobi.meeting.retardAide', { duree, arret });
    this.retard.setAttribute('aria-label', this.retard.title);
  }

  /** Le délai d'« Annuler » est passé : l'étiquette s'efface. */
  closeUndo(): void {
    if (!this.undoable) return;
    this.undoable = false;
    this.undo.hidden = true;
    this.time.hidden = false;
    this.root.setAttribute('aria-label', t('kobi.meeting.ariaTerminee'));
    this.fade();
  }

  get canUndo(): boolean {
    return this.undoable;
  }

  /** L'étiquette « réunion terminée » s'efface. */
  private fade(): void {
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
    this.undoable = false;
    this.undo.hidden = true;
    this.time.hidden = false;
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
    this.offLangue();
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
  private readonly title: HTMLElement;
  private readonly langs: HTMLElement;
  private readonly kicker: HTMLElement;
  private readonly intro: HTMLElement;
  private readonly done: HTMLButtonElement;
  /** Issue de la dernière copie (`null` : pas encore copié). */
  private copied: boolean | null = null;
  readonly offLangue: () => void;

  constructor() {
    this.root = el('div', 'card card-consent');
    this.root.dataset.card = 'consent';
    this.root.setAttribute('role', 'dialog');
    this.root.setAttribute('aria-labelledby', 'consent-titre');
    const title = (this.title = el('div', 'title'));
    title.id = 'consent-titre';
    const langs = (this.langs = el('div', 'langs'));
    langs.setAttribute('role', 'group');
    this.langButtons = { fr: el('button', 'lang', 'FR'), en: el('button', 'lang', 'EN') };
    for (const l of ['fr', 'en'] as const) {
      const b = this.langButtons[l];
      b.type = 'button';
      b.addEventListener('click', () => this.setLang(l));
      langs.append(b);
    }
    this.message = el('blockquote', 'message');
    this.copyButton = el('button', 'btn btn-primary');
    this.copyButton.type = 'button';
    this.copyButton.addEventListener('click', () => void this.copy());
    const done = (this.done = el('button', 'btn'));
    done.type = 'button';
    done.addEventListener('click', () => this.onEvent?.({ action: 'done', lang: this.lang }));
    const buttons = el('div', 'buttons');
    buttons.append(this.copyButton, done);
    this.kicker = el('div', 'kicker');
    this.intro = el('p', 'intro');
    this.root.append(this.kicker, title, this.intro, langs, this.message, buttons);
    this.setLang('fr');
    // La carte reste ouverte : elle suit la langue de l'interface (le message
    // à coller garde la langue choisie sur la carte).
    // Retirée par la scène (`root.remove()`) : elle se désabonne d'elle-même.
    this.offLangue = surLangue(() => (this.root.isConnected ? this.textes() : this.offLangue()));
  }

  private textes(): void {
    this.kicker.textContent = t('kobi.meeting.consentKicker');
    this.title.textContent = t('kobi.meeting.consentTitre');
    this.intro.textContent = t('kobi.meeting.consentIntro');
    this.langs.setAttribute('aria-label', t('kobi.meeting.consentLangue'));
    this.done.textContent = t('kobi.meeting.consentFait');
    this.copyButton.textContent =
      this.copied === null ? t('kobi.meeting.consentCopier') : this.copied ? t('kobi.meeting.consentCopie') : t('kobi.meeting.consentCopieImpossible');
  }

  setLang(lang: ConsentLang): void {
    this.lang = lang;
    this.message.textContent = CONSENT_MESSAGES[lang];
    this.message.lang = lang;
    for (const l of ['fr', 'en'] as const) this.langButtons[l].setAttribute('aria-pressed', String(l === lang));
    this.copied = null;
    this.textes();
  }

  private async copy(): Promise<void> {
    const ok = await copy(CONSENT_MESSAGES[this.lang]);
    this.copied = ok;
    this.textes();
    if (ok) this.onEvent?.({ action: 'copy', lang: this.lang });
  }
}
