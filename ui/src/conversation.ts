// Conversation (KOB-133) : champ de texte sous Kobi et historique court des
// échanges, qu'on fait défiler dans la bulle.
//
// La coque tient la conversation (voix et clavier, même session Claude) :
// l'interface envoie le texte tapé (`envoyer_texte`) et affiche l'historique
// que la coque publie (événement `conversation`, commande `historique` au
// chargement). Rien n'est gardé ici.
//
// Toujours animé, mais jamais l'élément cliquable lui-même : le champ ne bouge
// pas, ce sont son chevron, son curseur et son invite qui vivent.

import { tauri } from './native';
import type { Stage } from './stage';

/** Une bulle de l'historique, telle que la publie la coque. */
export interface Bulle {
  tour: number;
  question: string;
  reponse: string;
  /** `attente`, `reponse`, `tache` (tâche longue en cours), `fini`, `erreur`. */
  statut: 'attente' | 'reponse' | 'tache' | 'fini' | 'erreur';
  /** Question tapée au clavier. */
  clavier: boolean;
}

/** Fermeture automatique de l'historique sans interaction. */
const HISTORY_IDLE_MS = 12_000;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export class Conversation {
  readonly form: HTMLFormElement;
  readonly input: HTMLInputElement;
  readonly history: HTMLElement;
  /** Texte envoyé (Entrée). */
  onSend: ((text: string) => void) | null = null;
  private readonly list: HTMLElement;
  private bulles: Bulle[] = [];
  private idleTimer = 0;

  constructor(private readonly stage: Stage) {
    this.form = el('form', 'say');
    this.form.setAttribute('role', 'search');
    this.form.setAttribute('aria-label', 'Écrire à Kobi');
    const chevron = el('span', 'say-chevron', '›');
    chevron.setAttribute('aria-hidden', 'true');
    this.input = el('input', 'say-input');
    this.input.type = 'text';
    this.input.name = 'texte';
    this.input.autocomplete = 'off';
    this.input.spellcheck = false;
    this.input.placeholder = 'Écrire à Kobi…';
    this.input.setAttribute('aria-label', 'Écrire à Kobi');
    const enter = el('span', 'say-enter', '↵');
    enter.setAttribute('aria-hidden', 'true');
    this.form.append(chevron, this.input, enter);
    this.form.addEventListener('submit', (e) => {
      e.preventDefault();
      this.send();
    });
    this.input.addEventListener('input', () => this.form.classList.toggle('has-text', this.input.value.trim() !== ''));
    this.input.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowUp' && this.input.value === '' && this.bulles.length > 0) {
        e.preventDefault();
        this.openHistory();
      }
    });

    this.history = el('div', 'card card-history');
    this.history.dataset.panel = 'history';
    this.history.setAttribute('role', 'log');
    this.history.setAttribute('aria-label', 'Échanges récents avec Kobi');
    this.history.hidden = true;
    this.list = el('div', 'history-list');
    this.history.append(el('div', 'kicker', 'Échanges récents'), this.list);
    this.history.addEventListener('wheel', (e) => this.onHistoryWheel(e), { passive: true });
    this.history.addEventListener('pointermove', () => this.armIdle());

    stage.mount(this.history);
    stage.mount(this.form);
    // Faire défiler vers le haut, sur la bulle ou sur Kobi, ouvre l'historique.
    stage.root.addEventListener(
      'wheel',
      (e) => {
        if (this.history.hidden && e.deltaY < 0 && this.bulles.length > 0) this.openHistory();
      },
      { passive: true },
    );
    // Taper au clavier n'importe où écrit dans le champ.
    document.addEventListener('keydown', (e) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || e.key.length !== 1) return;
      const target = e.target as Element | null;
      if (target?.closest('input, textarea, select, [contenteditable]')) return;
      this.input.focus({ preventScroll: true });
    });
    const previous = stage.onStateChange;
    stage.onStateChange = (state) => {
      previous?.(state);
      // Kobi change d'état (nouvelle question, réponse…) : retour au présent.
      if (state !== 'idle' && state !== 'write') this.closeHistory();
    };
  }

  get historyOpen(): boolean {
    return !this.history.hidden;
  }

  /** Historique publié par la coque (du plus ancien au plus récent). */
  setBulles(bulles: readonly Bulle[]): void {
    this.bulles = [...bulles];
    this.renderHistory();
  }

  openHistory(): void {
    if (this.bulles.length === 0) return;
    this.history.hidden = false;
    this.stage.root.classList.add('history-open');
    this.list.scrollTop = this.list.scrollHeight;
    this.armIdle();
  }

  closeHistory(): void {
    if (this.history.hidden) return;
    window.clearTimeout(this.idleTimer);
    this.history.hidden = true;
    this.stage.root.classList.remove('history-open');
  }

  private send(): void {
    const text = this.input.value.trim();
    if (!text) return;
    this.input.value = '';
    this.form.classList.remove('has-text');
    this.closeHistory();
    // Réaction immédiate : la question s'affiche avant toute réponse de la coque.
    this.stage.setState('listen', { question: text });
    this.onSend?.(text);
  }

  private armIdle(): void {
    window.clearTimeout(this.idleTimer);
    this.idleTimer = window.setTimeout(() => this.closeHistory(), HISTORY_IDLE_MS);
  }

  /** Arrivé en bas de l'historique, défiler encore vers le bas le referme. */
  private onHistoryWheel(e: WheelEvent): void {
    this.armIdle();
    const atBottom = this.list.scrollTop + this.list.clientHeight >= this.list.scrollHeight - 2;
    if (e.deltaY > 0 && atBottom) this.closeHistory();
  }

  private renderHistory(): void {
    const wasAtBottom = this.list.scrollTop + this.list.clientHeight >= this.list.scrollHeight - 2;
    const seen = new Set<number>();
    for (const b of this.bulles) {
      seen.add(b.tour);
      let item = this.list.querySelector<HTMLElement>(`[data-tour="${b.tour}"]`);
      if (!item) {
        item = el('div', 'ex');
        item.dataset.tour = String(b.tour);
        const q = el('div', 'ex-q');
        q.append(el('span', 'ex-who'), el('span', 'ex-text'));
        item.append(q, el('div', 'ex-a'));
        this.list.append(item);
      }
      item.dataset.statut = b.statut;
      item.querySelector('.ex-who')!.textContent = b.clavier ? 'tapé' : 'dit';
      item.querySelector('.ex-q .ex-text')!.textContent = b.question;
      item.querySelector('.ex-a')!.textContent = b.reponse || (b.statut === 'attente' ? '…' : '');
    }
    for (const item of [...this.list.querySelectorAll<HTMLElement>('.ex')]) {
      if (!seen.has(Number(item.dataset.tour))) item.remove();
    }
    if (wasAtBottom) this.list.scrollTop = this.list.scrollHeight;
  }
}

/**
 * Branche la conversation sur la coque (rien hors de Tauri) ; `onBulles`
 * reçoit chaque historique publié (tâches longues).
 */
export function followConversation(conversation: Conversation, onBulles: (bulles: Bulle[]) => void): void {
  const api = tauri();
  if (!api) return;
  const show = (bulles: unknown): void => {
    if (!Array.isArray(bulles)) return;
    conversation.setBulles(bulles as Bulle[]);
    onBulles(bulles as Bulle[]);
  };
  conversation.onSend = (texte) => void api.core.invoke('envoyer_texte', { texte }).catch(() => {});
  void api.event.listen<Bulle[]>('conversation', (e) => show(e.payload));
  api.core.invoke<Bulle[]>('historique').then(show, () => {});
}
