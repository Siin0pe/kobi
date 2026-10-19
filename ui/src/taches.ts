// Tâches longues (KOB-135) : annoncées tout de suite, livrées plus tard dans
// une bulle de notification, sans couper ce qui est en cours.
//
// Aucun événement dédié : la coque passe la bulle de l'historique au statut
// `tache` quand elle détache un tour, puis à `fini` (résultat) ou `erreur`.
// Une notification « livrée » se clique pour afficher le résultat dans la
// bulle de réponse. Pendant que Kobi s'en occupe (état `work`), la
// notification « en cours » se tait : le fil sous Kobi suffit (KOB-152).

import type { Bulle } from './conversation';
import { tauri } from './native';
import { dealFromKobi, leaveToKobi } from './effets';
import type { Stage } from './stage';

/** Une notification de livraison reste affichée ce temps-là sans clic. */
const DELIVERED_MS = 20_000;
/** Délai pour voir si Kobi montre déjà le résultat dans sa bulle (la coque change d'état juste après l'historique). */
const DELIVER_CHECK_MS = 400;
/**
 * Sobriété (règle du propriétaire) : une seule notification visible à la fois.
 * Les autres attendent leur tour, par priorité (rappel, puis résultat ou
 * échec, puis annonce) ; au-delà de `MAX_NOTIFS` en attente, la plus ancienne
 * annonce s'en va.
 */
const MAX_NOTIFS = 4;
const RANG: Record<string, number> = { reminder: 0, delivered: 1, failed: 1, running: 2 };

type Kind = 'running' | 'delivered' | 'failed';

/** Rappel arrivé à l'heure dite (KOB-145), comme l'événement `rappel` de la coque. */
export interface Rappel {
  id: string;
  texte: string;
  echeance_ms: number | null;
}

/** Cloche dessinée (pas d'emoji : la police de Kobi n'en a pas). */
const BELL =
  '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path fill="currentColor" d="M12 2a1.6 1.6 0 0 0-1.6 1.6v.6A6.4 6.4 0 0 0 5.6 10.4v4.2L3.8 17.4h16.4l-1.8-2.8v-4.2a6.4 6.4 0 0 0-4.8-6.2v-.6A1.6 1.6 0 0 0 12 2Zm-2.6 16.6a2.6 2.6 0 0 0 5.2 0Z"/></svg>';

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
  private readonly timers = new Map<string, number>();
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
      else if (before === 'tache' && b.statut === 'fini') this.deliver(b);
      else if (before === 'tache' && b.statut === 'erreur') this.show(b, 'failed');
    }
    this.first = false;
  }

  /**
   * Résultat d'une tâche : Kobi le montre lui-même dans sa bulle s'il est
   * libre ; la notification ne sert que si la bulle est prise (Kobi écoute
   * une autre question), pour ne pas afficher deux fois la même réponse.
   */
  private deliver(b: Bulle): void {
    this.dismiss(b.tour);
    window.setTimeout(() => {
      if (this.stage.currentState !== 'answer') this.show(b, 'delivered');
    }, DELIVER_CHECK_MS);
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
    // Elle part de Kobi.
    dealFromKobi(n);
    this.keep();
    if (kind !== 'running') this.timers.set(String(b.tour), window.setTimeout(() => this.dismiss(b.tour), DELIVERED_MS));
  }

  /**
   * Rappel à l'heure dite : il reste jusqu'au clic (on ne rate pas un
   * rappel), la cloche sonne et Kobi le dit à voix haute (la coque).
   *
   * La vue agrandie de la conversation (KOB-169) cache les notifications le
   * temps de la lecture : un rappel la range d'abord, sans quoi il arrivait
   * caché (KOB-176). Pas de bulle « Rappel : … » en plus : elle répétait la
   * notification et, en réponse, rouvrait la vue.
   */
  reminder(r: Rappel): void {
    const key = `rappel-${r.id}`;
    this.dismiss(key);
    const n = el('button', 'notif notif-reminder');
    n.type = 'button';
    n.dataset.tour = key;
    n.dataset.kind = 'reminder';
    const icon = el('span', 'notif-icon');
    icon.innerHTML = BELL;
    const body = el('span', 'notif-body');
    const heure = r.echeance_ms === null ? '' : new Date(r.echeance_ms).toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' });
    body.append(el('span', 'notif-kicker', heure ? `Rappel · ${heure}` : 'Rappel'), el('span', 'notif-text', preview(r.texte)));
    n.append(icon, body);
    n.setAttribute('aria-label', `Rappel : ${r.texte}`);
    n.addEventListener('click', () => this.dismiss(key));
    window.dispatchEvent(new CustomEvent('kobi:vue', { detail: 'fermer' }));
    this.root.append(n);
    dealFromKobi(n);
    this.keep();
  }

  /**
   * Une seule notification visible : la plus prioritaire (la plus ancienne à
   * rang égal). Les autres attendent, cachées ; celle qui prend la place sort
   * de Kobi. Celles qui partent ne comptent plus.
   */
  private keep(): void {
    const all = () => [...this.root.querySelectorAll<HTMLElement>('.notif[data-tour]')];
    while (all().length > MAX_NOTIFS) {
      const annonce = all().find((n) => n.dataset.kind === 'running') ?? all()[0]!;
      this.dismiss(annonce.dataset.tour ?? '');
    }
    const rang = (n: HTMLElement) => RANG[n.dataset.kind ?? 'running'] ?? 2;
    const visible = [...all()].sort((a, b) => rang(a) - rang(b))[0];
    for (const n of all()) {
      const attend = n !== visible;
      const attendait = n.classList.contains('en-attente');
      n.classList.toggle('en-attente', attend);
      if (attendait && !attend) dealFromKobi(n);
    }
  }

  /** La notification retourne dans Kobi, puis quitte le DOM. */
  private dismiss(tour: number | string): void {
    const key = String(tour);
    window.clearTimeout(this.timers.get(key));
    this.timers.delete(key);
    const n = this.root.querySelector<HTMLElement>(`[data-tour="${key}"]`);
    if (!n) return;
    delete n.dataset.tour;
    // Une notification en attente n'était pas visible : elle part sans effet.
    if (n.classList.contains('en-attente')) n.remove();
    else {
      n.classList.add('leaving');
      leaveToKobi(n, () => n.remove());
    }
    this.keep();
  }
}

/** Rappels de la coque (événement `rappel`, KOB-145). */
export function followRappels(tasks: Tasks): void {
  void tauri()?.event.listen<Rappel>('rappel', (e) => tasks.reminder(e.payload));
}
