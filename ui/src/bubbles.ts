// Bulles et cartes de Kobi : question, sources consultées,
// fiche de notes, réponse, carte d'action, carte d'erreur. Elles prennent les
// couleurs et la police de Kobi (variables CSS de la scène). Elles sortent de
// Kobi et y retournent (effets.ts) ; pendant la prise de notes, les lettres de
// chaque nouvelle note volent de Kobi vers la fiche, où un crayon gratte.

import { dealFromKobi, flyLetters, leaveToKobi } from './effets';
import { Defilement } from './defilement';
import type { ActionCard, MascotData, MascotState } from './states';
import { Typewriter } from './typewriter';

export interface ActionDecision {
  id: string;
  decision: 'confirm' | 'dismiss';
}

type Kind = 'question' | 'sources' | 'notes' | 'answer' | 'action' | 'error' | 'compose';

/** Vitesses de frappe du prototype, en caractères par seconde. */
const TYPING_CPS: Partial<Record<Kind, number>> = { question: 18, notes: 22, answer: 25, compose: 22 };

/** Bulle de la rédaction du compte rendu (KOB-166). */
export const COMPOSE_TEXT = 'Je rédige le compte rendu…';



/** Carte visible pour un état donné, ou rien. */
export function cardKind(state: MascotState, data: MascotData): Kind | null {
  switch (state) {
    case 'listen':
      return 'question';
    case 'think':
      return 'sources';
    case 'write':
      return 'notes';
    case 'answer':
      return 'answer';
    case 'suggest':
    case 'done':
      return data.action ? 'action' : null;
    case 'error':
      return data.error ? 'error' : null;
    case 'compose':
      return 'compose';
    default:
      return null;
  }
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export class Bubbles {
  readonly root: HTMLElement;
  onAction: ((decision: ActionDecision) => void) | null = null;
  /** Lire la réponse en grand (vue agrandie, KOB-169) : clic sur la bulle ou sur « Lire en grand ». */
  onReadAll: (() => void) | null = null;
  private scroll: Defilement | null = null;
  private kind: Kind | null = null;
  private card: HTMLElement | null = null;
  private actionKey = '';
  private typer: Typewriter | null = null;
  private reduced = false;
  /** Texte de la fiche de notes déjà montré (lettres qui volent à la note suivante). */
  private notesShown = '';

  constructor() {
    this.root = el('div', 'bubbles');
  }

  render(state: MascotState, data: MascotData): void {
    const kind = cardKind(state, data);
    if (kind !== this.kind) {
      if (this.card) this.leave(this.card);
      this.typer?.stop();
      this.typer = null;
      this.scroll?.destroy();
      this.scroll = null;
      this.card = kind ? this.create(kind) : null;
      this.kind = kind;
      this.actionKey = '';
      this.notesShown = '';
      if (this.card) {
        this.root.append(this.card);
        dealFromKobi(this.card);
      }
    }
    if (this.card && kind) this.fill(kind, this.card, data);
  }

  /** Interruption (Échap) : la frappe s'arrête net, le texte affiché reste. */
  interrupt(): void {
    this.typer?.freeze();
    if (this.card) this.card.dataset.typing = '0';
  }

  setPaused(paused: boolean): void {
    this.root.classList.toggle('paused', paused);
  }

  setReducedMotion(reduced: boolean): void {
    this.reduced = reduced;
    this.root.classList.toggle('reduced-motion', reduced);
    this.typer?.setInstant(reduced);
    this.scroll?.setInstant(reduced);
  }

  /** La carte remplacée s'efface (sans bloquer la nouvelle) puis disparaît du DOM. */
  private leave(card: HTMLElement): void {
    delete card.dataset.card;
    if (this.reduced) {
      card.remove();
      return;
    }
    card.classList.add('leaving');
    leaveToKobi(card, () => card.remove());
  }

  private typed(card: HTMLElement, kind: Kind, text: string): void {
    if (!this.typer) {
      const node = card.querySelector('.text')!;
      card.dataset.typing = '1';
      this.typer = new Typewriter(node, TYPING_CPS[kind] ?? 25, () => (card.dataset.typing = '0'), kind === 'answer');
      this.typer.setInstant(this.reduced);
    }
    if (text.length > (card.querySelector('.text')!.textContent ?? '').length) card.dataset.typing = '1';
    this.typer.set(text);
  }

  private create(kind: Kind): HTMLElement {
    const card = el('div', `card card-${kind}`);
    card.dataset.card = kind;
    switch (kind) {
      case 'question':
        card.append(el('span', 'lead', 'Kobi,'), document.createTextNode(' '), el('span', 'text'), el('span', 'caret'));
        break;
      case 'sources':
        break;
      case 'notes':
        card.append(el('div', 'kicker', 'Notes · en cours'), el('div', 'body'));
        card.querySelector('.body')!.append(el('span', 'text'), el('span', 'caret'), el('span', 'crayon', '✎'));
        card.querySelector('.crayon')!.setAttribute('aria-hidden', 'true');
        break;
      case 'answer': {
        // Le texte mis en forme défile dans sa zone ; les étiquettes et le
        // lien vers l'app restent en dessous, toujours visibles.
        const zone = el('div', 'defile');
        const text = el('div', 'text md');
        zone.append(text, el('span', 'caret'));
        // Montré quand la bulle ne peut pas tout montrer (reponse.css).
        const all = el('button', 'lire-tout', 'Lire en grand ›');
        all.type = 'button';
        all.addEventListener('click', (e) => {
          e.stopPropagation();
          this.onReadAll?.();
        });
        // Toute la bulle s'ouvre en grand d'un clic (sauf après un glisser pour faire défiler).
        card.addEventListener('click', () => this.onReadAll?.());
        card.append(zone, el('div', 'tags'), all);
        this.scroll = new Defilement(zone, text);
        this.scroll.setInstant(this.reduced);
        break;
      }
      case 'action':
        break;
      case 'error':
        card.append(el('div', 'kicker'), el('div', 'message'));
        break;
      case 'compose':
        card.append(el('div', 'kicker', 'Réunion terminée'), el('div', 'body'));
        card.querySelector('.body')!.append(el('span', 'text'), el('span', 'caret'));
        break;
    }
    return card;
  }

  private fill(kind: Kind, card: HTMLElement, data: MascotData): void {
    switch (kind) {
      case 'question':
        this.typed(card, kind, data.question ?? '');
        break;
      case 'notes': {
        const notes = data.notes ?? '';
        // Une note de plus (pas la première fiche) : ses lettres volent de Kobi vers la fiche.
        if (this.notesShown && notes !== this.notesShown) flyLetters(this.root, card.querySelector('.body')!, notes.split('\n').pop() ?? notes);
        this.notesShown = notes;
        this.typed(card, kind, notes);
        break;
      }
      case 'compose':
        this.typed(card, kind, COMPOSE_TEXT);
        break;
      case 'answer': {
        this.typed(card, kind, data.answer ?? '');
        const tags = card.querySelector<HTMLElement>('.tags')!;
        const wanted = (data.answerTags ?? []).join('\u0000');
        if (tags.dataset.key !== wanted) {
          tags.dataset.key = wanted;
          tags.replaceChildren(...(data.answerTags ?? []).map((t) => el('span', 'tag', t)));
        }
        break;
      }
      case 'sources':
        this.fillSources(card, data);
        break;
      case 'action':
        this.fillAction(card, data.action!);
        break;
      case 'error':
        setText(card.querySelector('.kicker')!, data.error?.title ?? '');
        setText(card.querySelector('.message')!, data.error?.message ?? '');
        break;
    }
  }

  private fillSources(card: HTMLElement, data: MascotData): void {
    const sources = data.sources ?? [];
    // Les sources s'ajoutent une à une : on garde les pastilles déjà posées
    // pour ne pas rejouer leur apparition.
    while (card.children.length > sources.length) card.lastElementChild!.remove();
    sources.forEach((s, i) => {
      let pill = card.children[i] as HTMLElement | undefined;
      if (!pill) {
        pill = el('div', 'card source');
        pill.append(el('span', 'icon'), el('span', 'label'));
        card.append(pill);
        dealFromKobi(pill);
      }
      pill.classList.toggle('found', s.found);
      setText(pill.querySelector('.icon')!, s.found ? '→' : (s.icon ?? '·'));
      setText(pill.querySelector('.label')!, s.label + (s.found ? ' · trouvé' : ' …'));
    });
  }

  private fillAction(card: HTMLElement, action: ActionCard): void {
    const key = `${action.id}:${action.status}`;
    if (key === this.actionKey) return;
    this.actionKey = key;
    card.dataset.status = action.status;
    if (action.status === 'pending') {
      const confirm = el('button', 'btn btn-primary', 'Créer');
      const dismiss = el('button', 'btn', 'Pas maintenant');
      confirm.type = 'button';
      dismiss.type = 'button';
      confirm.addEventListener('click', () => this.onAction?.({ id: action.id, decision: 'confirm' }));
      dismiss.addEventListener('click', () => this.onAction?.({ id: action.id, decision: 'dismiss' }));
      const buttons = el('div', 'buttons');
      buttons.append(confirm, dismiss);
      card.replaceChildren(el('div', 'kicker', action.kicker), el('div', 'title', action.title));
      if (action.meta) card.append(el('div', 'meta', action.meta));
      card.append(buttons);
    } else {
      const done = action.status === 'confirmed';
      card.replaceChildren(
        el('div', 'kicker', done ? (action.doneKicker ?? action.kicker) : action.kicker),
        el('div', 'title', done ? (action.doneTitle ?? 'C’est fait.') : 'Pas maintenant.'),
      );
      const meta = done ? action.doneMeta : 'Gardé dans le compte rendu.';
      if (meta) card.append(el('div', 'meta', meta));
    }
  }
}

function setText(node: Element, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}
