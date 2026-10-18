// Conversation (KOB-133) : historique court des échanges, qu'on fait défiler
// dans la bulle. On écrit à Kobi dans sa bulle d'écriture (ecrire.ts, KOB-147).
//
// La coque tient la conversation (voix et clavier, même session Claude) :
// l'interface envoie le texte tapé (`envoyer_texte`) et affiche l'historique
// que la coque publie (événement `conversation`, commande `historique` au
// chargement). Rien n'est gardé ici.

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
  readonly history: HTMLElement;
  /** Texte envoyé (Entrée). */
  onSend: ((text: string) => void) | null = null;
  private readonly list: HTMLElement;
  private bulles: Bulle[] = [];
  private idleTimer = 0;

  constructor(private readonly stage: Stage) {
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
    // Faire défiler vers le haut, sur la bulle ou sur Kobi, ouvre l'historique.
    stage.root.addEventListener(
      'wheel',
      (e) => {
        if (this.history.hidden && e.deltaY < 0 && this.bulles.length > 0) this.openHistory();
      },
      { passive: true },
    );
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

  /** Texte tapé dans la bulle d'écriture, déjà avalé par Kobi : il part à la coque. */
  send(text: string): void {
    this.closeHistory();
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
