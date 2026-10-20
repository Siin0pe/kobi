// Conversation (KOB-133) : historique court des échanges, qu'on fait défiler
// dans la bulle. On écrit à Kobi dans sa bulle d'écriture (ecrire.ts, KOB-147).
//
// La coque tient la conversation (voix et clavier, même session Claude) :
// l'interface envoie le texte tapé (`envoyer_texte`) et affiche l'historique
// que la coque publie (événement `conversation`, commande `historique` au
// chargement). Rien n'est gardé ici.

import { etapeLigne, type Etape } from './activite';
import { dealFromKobi, leaveToKobi, reduced } from './effets';
import { surLangue, t } from './i18n';
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
  /** Étapes du tour (outils appelés, KOB-152), repliées sous la réponse. */
  etapes?: Etape[];
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
  /** L'historique retourne dans Kobi (sortie animée en cours). */
  private closing = false;

  constructor(private readonly stage: Stage) {
    this.history = el('div', 'card card-history');
    this.history.dataset.panel = 'history';
    this.history.setAttribute('role', 'log');
    this.history.hidden = true;
    this.list = el('div', 'history-list');
    const kicker = el('div', 'kicker');
    this.history.append(kicker, this.list);
    // L'historique peut rester ouvert : ses textes suivent la langue.
    const traduire = (): void => {
      this.history.setAttribute('aria-label', t('kobi.conversation.aria'));
      kicker.textContent = t('kobi.conversation.titre');
      for (const box of this.list.querySelectorAll<HTMLElement>('.ex-steps')) delete box.dataset.n;
      this.renderHistory();
    };
    traduire();
    surLangue(traduire);
    this.history.addEventListener('wheel', (e) => this.onHistoryWheel(e), { passive: true });
    this.history.addEventListener('pointermove', () => this.armIdle());

    stage.mount(this.history);
    // Faire défiler vers le haut, sur la bulle ou sur Kobi, ouvre l'historique.
    stage.root.addEventListener(
      'wheel',
      (e) => {
        // La vue agrandie et le champ d'écriture gardent leur molette :
        // l'historique, caché sous eux, ne s'ouvrait que pour surgir ensuite
        // (KOB-246).
        if (stage.root.classList.contains('vue-ouverte') || (e.target as Element | null)?.closest('.lecture, .ecrire')) return;
        // Une réponse longue qui défile garde sa molette (KOB-163).
        const zone = (e.target as Element | null)?.closest<HTMLElement>('.card-answer .defile');
        if (zone && zone.scrollHeight > zone.clientHeight + 2 && zone.scrollTop > 0) return;
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
    return !this.history.hidden && !this.closing;
  }

  /** Historique publié par la coque (du plus ancien au plus récent). */
  setBulles(bulles: readonly Bulle[]): void {
    this.bulles = [...bulles];
    this.renderHistory();
  }

  /** L'historique sort de Kobi ; ses échanges entrent en cascade, du plus récent au plus ancien. */
  openHistory(): void {
    if (this.bulles.length === 0) return;
    const reopen = this.closing;
    this.closing = false;
    this.history.getAnimations().forEach((a) => a.cancel());
    const wasHidden = this.history.hidden;
    this.history.hidden = false;
    this.stage.root.classList.add('history-open');
    this.list.scrollTop = this.list.scrollHeight;
    this.armIdle();
    if (!wasHidden && !reopen) return;
    dealFromKobi(this.history);
    if (reduced(this.history)) return;
    const items = [...this.list.querySelectorAll<HTMLElement>('.ex')].reverse();
    items.slice(0, 8).forEach((item, i) =>
      item.animate(
        [
          { transform: 'translate(-10px, 12px) rotate(-2deg)', opacity: 0 },
          { transform: 'translate(2px, -2px) rotate(.5deg)', opacity: 1, offset: 0.7 },
          { transform: 'none', opacity: 1 },
        ],
        { duration: 380, delay: 160 + i * 60, easing: 'cubic-bezier(.2,.8,.3,1)', fill: 'backwards' },
      ),
    );
  }

  /** L'historique retourne dans Kobi ; les bulles reviennent tout de suite. */
  closeHistory(): void {
    if (this.history.hidden || this.closing) return;
    window.clearTimeout(this.idleTimer);
    this.stage.root.classList.remove('history-open');
    this.closing = true;
    leaveToKobi(this.history, () => {
      if (!this.closing) return;
      this.closing = false;
      this.history.hidden = true;
      this.history.getAnimations().forEach((a) => a.cancel());
    });
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
      item.querySelector('.ex-who')!.textContent = b.clavier ? t('kobi.conversation.tape') : t('kobi.conversation.dit');
      item.querySelector('.ex-q .ex-text')!.textContent = b.question;
      item.querySelector('.ex-a')!.textContent = b.reponse || (b.statut === 'attente' ? '…' : '');
      this.renderSteps(item, b.etapes ?? []);
    }
    for (const item of [...this.list.querySelectorAll<HTMLElement>('.ex')]) {
      if (!seen.has(Number(item.dataset.tour))) item.remove();
    }
    if (wasAtBottom) this.list.scrollTop = this.list.scrollHeight;
  }

  /** Étapes du tour, repliées sous la réponse ; on les déplie d'un clic. */
  private renderSteps(item: HTMLElement, etapes: readonly Etape[]): void {
    let box = item.querySelector<HTMLDetailsElement>('.ex-steps');
    if (etapes.length === 0) {
      box?.remove();
      return;
    }
    if (!box) {
      box = el('details', 'ex-steps');
      box.append(el('summary'), el('div', 'ex-steps-list'));
      item.append(box);
    }
    if (box.dataset.n === String(etapes.length)) return;
    box.dataset.n = String(etapes.length);
    box.querySelector('summary')!.textContent = t('kobi.conversation.etapes', { n: etapes.length });
    box.querySelector('.ex-steps-list')!.replaceChildren(...etapes.map((e) => etapeLigne(e)));
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
