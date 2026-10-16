// Bulles et cartes communes à tous les skins : question, sources consultées,
// fiche de notes, réponse, carte d'action, carte d'erreur. Elles prennent les
// couleurs et la police du thème du skin (variables CSS posées par la scène).

import type { ActionCard, MascotData, MascotState } from './contract';

export interface ActionDecision {
  id: string;
  decision: 'confirm' | 'dismiss';
}

type Kind = 'question' | 'sources' | 'notes' | 'answer' | 'action' | 'error';

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
  private kind: Kind | null = null;
  private card: HTMLElement | null = null;
  private actionKey = '';

  constructor() {
    this.root = el('div', 'bubbles');
  }

  render(state: MascotState, data: MascotData): void {
    const kind = cardKind(state, data);
    if (kind !== this.kind) {
      this.card?.remove();
      this.card = kind ? this.create(kind) : null;
      this.kind = kind;
      this.actionKey = '';
      if (this.card) this.root.append(this.card);
    }
    if (this.card && kind) this.fill(kind, this.card, data);
  }

  setPaused(paused: boolean): void {
    this.root.classList.toggle('paused', paused);
  }

  setReducedMotion(reduced: boolean): void {
    this.root.classList.toggle('reduced-motion', reduced);
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
        card.querySelector('.body')!.append(el('span', 'text'), el('span', 'caret'));
        break;
      case 'answer':
        card.append(el('span', 'text'), el('span', 'caret'), el('div', 'tags'));
        break;
      case 'action':
        break;
      case 'error':
        card.append(el('div', 'kicker'), el('div', 'message'));
        break;
    }
    return card;
  }

  private fill(kind: Kind, card: HTMLElement, data: MascotData): void {
    switch (kind) {
      case 'question':
        setText(card.querySelector('.text')!, data.question ?? '');
        break;
      case 'notes':
        setText(card.querySelector('.text')!, data.notes ?? '');
        break;
      case 'answer': {
        setText(card.querySelector('.text')!, data.answer ?? '');
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
