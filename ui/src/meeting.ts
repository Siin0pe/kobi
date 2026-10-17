// Réunion : indicateur REC visible pendant toute la réunion et carte qui
// rappelle de prévenir les participants, avec un message prêt à coller (FR et
// EN). Communs à tous les skins, aux couleurs du thème (RGPD : les
// participants doivent savoir qu'ils sont transcrits).

import { CONSENT_MESSAGES, type ConsentLang, type MeetingInfo } from './contract';

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

/** Indicateur « ● REC · titre · 12:34 », mis à jour une fois par seconde. */
export class RecIndicator {
  readonly root: HTMLElement;
  private readonly label: HTMLElement;
  private readonly time: HTMLElement;
  private info: MeetingInfo | null = null;
  private timer = 0;
  private paused = false;

  constructor() {
    this.root = el('div', 'rec');
    this.root.hidden = true;
    // Pas de région « live » : la durée change chaque seconde et serait lue en
    // boucle. La scène annonce le début, la pause et la fin de la réunion.
    this.time = el('span', 'rec-time');
    this.time.setAttribute('aria-hidden', 'true');
    this.root.append(el('span', 'rec-dot'), (this.label = el('span', 'rec-label')), this.time);
  }

  get active(): boolean {
    return this.info !== null;
  }

  set(info: MeetingInfo | null): void {
    this.info = info;
    this.root.hidden = info === null;
    this.root.classList.toggle('is-paused', Boolean(info?.paused));
    if (info) {
      const word = info.paused ? 'PAUSE' : 'REC';
      this.label.textContent = info.title ? `${word} · ${info.title}` : word;
      this.root.setAttribute('aria-label', info.paused ? 'Transcription en pause' : 'Réunion transcrite en local');
    }
    this.tick();
  }

  /** Fenêtre masquée : plus de minuterie ; la durée est recalculée au retour. */
  setPaused(paused: boolean): void {
    this.paused = paused;
    this.tick();
  }

  destroy(): void {
    window.clearTimeout(this.timer);
  }

  private tick(): void {
    window.clearTimeout(this.timer);
    this.timer = 0;
    if (!this.info) return;
    const ms = Date.now() - this.info.startedAt;
    const text = elapsed(ms);
    if (this.time.textContent !== text) this.time.textContent = text;
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
