// Tâches longues (KOB-135) : annoncées tout de suite, livrées plus tard dans
// une bulle de notification, sans couper ce qui est en cours.
//
// Aucun événement dédié : la coque passe la bulle de l'historique au statut
// `tache` quand elle détache un tour, puis à `fini` (résultat) ou `erreur`.
// Une notification « livrée » se clique pour afficher le résultat dans la
// bulle de réponse.

import type { Bulle } from './conversation';
import type { Stage } from './stage';

/** Une notification de livraison reste affichée ce temps-là sans clic. */
const DELIVERED_MS = 20_000;
/** Au plus deux notifications à la fois (la plus ancienne s’en va) : elles tiennent au-dessus des bulles. */
const MAX_NOTIFS = 2;

type Kind = 'running' | 'delivered' | 'failed';

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Début de texte, pour un aperçu d'une ligne. */
function preview(text: string, max = 90): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

export class Tasks {
  readonly root: HTMLElement;
  private readonly statuts = new Map<number, Bulle['statut']>();
  private readonly timers = new Map<number, number>();
  private first = true;

  constructor(private readonly stage: Stage) {
    this.root = el('div', 'notifs');
    this.root.setAttribute('aria-live', 'polite');
    stage.mount(this.root);
  }

  /** Nouvel historique : chaque changement de statut d'une tâche longue est notifié. */
  update(bulles: readonly Bulle[]): void {
    for (const b of bulles) {
      const before = this.statuts.get(b.tour);
      this.statuts.set(b.tour, b.statut);
      // Au chargement, seules les tâches encore en cours sont rappelées.
      if (this.first) {
        if (b.statut === 'tache') this.show(b, 'running');
        continue;
      }
      if (b.statut === before) continue;
      if (b.statut === 'tache') this.show(b, 'running');
      else if (before === 'tache' && b.statut === 'fini') this.show(b, 'delivered');
      else if (before === 'tache' && b.statut === 'erreur') this.show(b, 'failed');
    }
    this.first = false;
  }

  private show(b: Bulle, kind: Kind): void {
    this.dismiss(b.tour);
    const n = el('button', `notif notif-${kind}`);
    n.type = 'button';
    n.dataset.tour = String(b.tour);
    n.dataset.kind = kind;
    const icon = el('span', 'notif-icon');
    icon.setAttribute('aria-hidden', 'true');
    if (kind === 'running') for (const c of '···') icon.append(el('i', undefined, c));
    else icon.textContent = kind === 'delivered' ? '✓' : '!';
    const body = el('span', 'notif-body');
    const kicker = { running: 'Je m’en occupe', delivered: 'Résultat prêt', failed: 'Tâche en échec' }[kind];
    body.append(el('span', 'notif-kicker', kicker), el('span', 'notif-text', preview(b.question)));
    if (kind !== 'running' && b.reponse) body.append(el('span', 'notif-preview', preview(b.reponse, 120)));
    n.append(icon, body);
    n.setAttribute('aria-label', `${kicker} : ${b.question}`);
    n.addEventListener('click', () => {
      if (kind !== 'running') this.stage.setState(kind === 'delivered' ? 'answer' : 'error', kind === 'delivered' ? { answer: b.reponse } : { error: { title: 'Tâche en échec', message: b.reponse } });
      this.dismiss(b.tour);
    });
    this.root.append(n);
    while (this.root.children.length > MAX_NOTIFS) this.dismiss(Number((this.root.firstElementChild as HTMLElement).dataset.tour));
    if (kind !== 'running') this.timers.set(b.tour, window.setTimeout(() => this.dismiss(b.tour), DELIVERED_MS));
  }

  private dismiss(tour: number): void {
    window.clearTimeout(this.timers.get(tour));
    this.timers.delete(tour);
    this.root.querySelector(`[data-tour="${tour}"]`)?.remove();
  }
}
