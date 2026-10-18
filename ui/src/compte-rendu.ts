// Compte rendu de fin de réunion (KOB-74) : « ouvert ou montré d'un clic ».
//
// Quand la coque a écrit `compte-rendu.md`, elle émet `compte_rendu` avec le
// résumé et le nombre de décisions, d'actions et de questions. Kobi le montre
// dans une carte ; « Ouvrir dans le Carnet » appelle `ouvrir_carnet
// { reunion }`, qui ouvre le Carnet sur ce compte rendu. « Plus tard », ou une
// nouvelle question, replie la carte en une pastille qui reste à portée de
// clic ; une nouvelle réunion la retire.
//
// Toujours animé, sans faire bouger ce qui se clique : la carte et ses
// boutons restent en place, ce sont le surtitre, les pastilles et le halo du
// bouton principal qui vivent.

import { tauri } from './native';
import type { Stage } from './stage';

/** Charge utile de l'événement `compte_rendu`. */
export interface CompteRenduUi {
  /** Nom de la réunion dans le stockage (son dossier). */
  reunion: string;
  resume: string;
  decisions: number;
  actions: number;
  questions: number;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** « 2 décisions », « 1 action », « aucune question ». */
function compte(n: number, mot: string, feminin: boolean): string {
  if (n === 0) return `${feminin ? 'aucune' : 'aucun'} ${mot}`;
  return `${n} ${mot}${n > 1 ? 's' : ''}`;
}

export class ReportCard {
  readonly root: HTMLElement;
  readonly pill: HTMLButtonElement;
  /** Ouvrir le compte rendu dans le Carnet. */
  onOpen: ((reunion: string) => void) | null = null;
  private current: CompteRenduUi | null = null;
  private readonly resume: HTMLElement;
  private readonly chips: HTMLElement;

  constructor(private readonly stage: Stage) {
    this.root = el('div', 'card card-rapport');
    this.root.dataset.panel = 'rapport';
    this.root.setAttribute('role', 'dialog');
    this.root.setAttribute('aria-labelledby', 'rapport-titre');
    this.root.hidden = true;
    const titre = el('div', 'title', 'Compte rendu prêt');
    titre.id = 'rapport-titre';
    this.resume = el('p', 'rapport-resume');
    this.chips = el('div', 'rapport-chips');
    const ouvrir = el('button', 'btn btn-primary', 'Ouvrir dans le Carnet');
    ouvrir.type = 'button';
    ouvrir.addEventListener('click', () => this.open());
    const plusTard = el('button', 'btn', 'Plus tard');
    plusTard.type = 'button';
    plusTard.addEventListener('click', () => this.collapse());
    const buttons = el('div', 'buttons');
    buttons.append(ouvrir, plusTard);
    this.root.append(el('div', 'kicker', 'Réunion terminée'), titre, this.resume, this.chips, buttons);

    this.pill = el('button', 'rapport-pastille');
    this.pill.type = 'button';
    this.pill.hidden = true;
    const icone = el('span', 'rapport-icone', '✓');
    icone.setAttribute('aria-hidden', 'true');
    this.pill.append(icone, el('span', 'rapport-pastille-texte', 'Compte rendu prêt'));
    this.pill.setAttribute('aria-label', 'Ouvrir le compte rendu dans le Carnet');
    this.pill.addEventListener('click', () => this.open());

    stage.mount(this.root);
    stage.mount(this.pill);
    const previous = stage.onStateChange;
    stage.onStateChange = (state) => {
      previous?.(state);
      // Une nouvelle question passe devant : la carte se replie en pastille.
      if (state !== 'idle' && state !== 'write' && !this.root.hidden) this.collapse();
    };
  }

  /** Compte rendu publié par la coque. */
  show(c: CompteRenduUi): void {
    this.current = c;
    this.resume.textContent = c.resume || 'Le compte rendu est rangé avec la réunion.';
    this.chips.replaceChildren(
      ...[compte(c.decisions, 'décision', true), compte(c.actions, 'action', true), compte(c.questions, 'question', true)].map((t) => el('span', 'rapport-chip', t)),
    );
    this.pill.hidden = true;
    this.root.hidden = false;
    this.stage.root.classList.add('rapport-open');
    this.stage.announce('Compte rendu de la réunion prêt.');
    this.root.querySelector<HTMLButtonElement>('.btn-primary')?.focus({ preventScroll: true });
  }

  /** Replie la carte en pastille. */
  collapse(): void {
    if (!this.current) return;
    this.root.hidden = true;
    this.stage.root.classList.remove('rapport-open');
    this.pill.hidden = false;
  }

  /** Retire la carte et la pastille (compte rendu ouvert, nouvelle réunion). */
  dismiss(): void {
    this.current = null;
    this.root.hidden = true;
    this.pill.hidden = true;
    this.stage.root.classList.remove('rapport-open');
  }

  private open(): void {
    if (!this.current) return;
    this.onOpen?.(this.current.reunion);
    this.dismiss();
  }
}

/** Branche la carte sur la coque (rien hors de Tauri). */
export function followReport(card: ReportCard): void {
  const api = tauri();
  if (!api) return;
  card.onOpen = (reunion) => void api.core.invoke('ouvrir_carnet', { reunion }).catch(() => {});
  void api.event.listen<CompteRenduUi>('compte_rendu', (e) => card.show(e.payload));
  // Une nouvelle réunion commence : l'ancien compte rendu reste dans le Carnet.
  void api.event.listen<{ actif: boolean }>('enregistrement', (e) => e.payload.actif && card.dismiss());
}
