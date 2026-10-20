// Fil d'activité (KOB-152) : sous Kobi, une seule ligne courte, l'étape en
// cours de son travail (fichier lu, recherche, commande, appel MCP), qui se
// remplace en douceur à chaque nouvelle étape. Sobriété : pas de carte ni de
// fond, juste le texte, comme le mot de Kobi ; le détail de toutes les étapes
// est dans l'app (section Activité).
//
// La coque émet l'événement `activite` pour chaque outil appelé par Claude,
// déjà résumé en une phrase (chemins relatifs au dossier de code, secrets
// masqués) ; ici, un chemin se réduit au nom du fichier. La ligne arrive
// brouillée, avec une petite icône animée selon l'outil, et la mascotte prend
// le geste de l'outil. « Réduire les animations » : même texte, sans mouvement.

import { surLangue, t } from './i18n';
import { tauri } from './native';
import { scramble, settleStep } from './mascotte/glyphs';
import type { Stage } from './stage';
import type { MascotState, ToolKind } from './states';

/** Une étape, telle que l'émet la coque (contrat commun des permissions, section 5). */
export interface Etape {
  tour: number;
  id: string;
  outil: string;
  type: ToolKind;
  resume: string;
  chemin_relatif: string | null;
  classe: 'lecture' | 'ecriture' | 'refus';
  decision?: 'accepte' | 'refuse' | 'delai' | null;
  /** Appel reçu de Claude (ms depuis l'époque Unix). */
  at_ms: number;
}

/** Sortie de la ligne remplacée. */
const LEAVE_MS = 320;
/** Pas du brouillage, comme le mot de Kobi. */
const SCRAMBLE_STEP_MS = 45;
/** Durée maximale du brouillage d'une ligne. */
const SETTLE_MS = 500;
/** États où le fil se montre : Kobi travaille (ou vient de lancer la question). */
const SHOWN: ReadonlySet<MascotState> = new Set(['work', 'think']);

/** Icône de chaque famille d'outil (symboles de la police de Kobi). */
export const ICONS: Record<ToolKind, string> = {
  lecture: '≡',
  recherche: '?',
  commande: '>',
  mcp: '+',
  ecriture: '/',
  reglage: '*',
};

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export function isEtape(value: unknown): value is Etape {
  const e = value as Etape | null;
  return typeof e === 'object' && e !== null && typeof e.tour === 'number' && typeof e.resume === 'string' && typeof e.type === 'string';
}

/** Ligne d'une étape (icône + texte), partagée par le fil, la notification et l'historique. */
export function etapeLigne(e: Etape, className = 'step'): HTMLElement {
  const line = el('div', className);
  line.dataset.type = e.type;
  line.dataset.classe = e.classe;
  line.dataset.id = e.id;
  const icon = el('span', 'step-icon', ICONS[e.type] ?? '·');
  icon.setAttribute('aria-hidden', 'true');
  const text = el('span', 'step-text', e.resume);
  line.append(icon, text);
  if (e.classe === 'refus') line.title = t('activite.refus');
  return line;
}

/** Longueur au-delà de laquelle une recherche perd son « dans … ». */
const COURTE_MAX = 34;

/**
 * Résumé court pour la ligne sous Kobi : un chemin se réduit au nom du
 * fichier, une longue recherche perd son dossier.
 */
export function courte(e: Etape): string {
  let texte = e.resume;
  const chemin = e.chemin_relatif;
  if (chemin && chemin.includes('/')) texte = texte.replace(chemin, chemin.split('/').filter(Boolean).at(-1) ?? chemin);
  if (e.type === 'recherche' && texte.length > COURTE_MAX) texte = texte.replace(/ dans \S+$/, '');
  return texte;
}

export class Activite {
  readonly root: HTMLElement;
  private readonly list: HTMLElement;
  private tour = -1;
  private tool: ToolKind | undefined;
  private readonly scrambles = new Map<HTMLElement, number>();
  /** Appelé quand une ligne est dessinée (mesure du délai, KOB-152). */
  onShown: ((e: Etape) => void) | null = null;

  constructor(private readonly stage: Stage) {
    this.root = el('div', 'fil');
    this.root.setAttribute('role', 'log');
    this.root.setAttribute('aria-live', 'polite');
    this.root.setAttribute('aria-label', t('kobi.activite.aria'));
    surLangue(() => this.root.setAttribute('aria-label', t('kobi.activite.aria')));
    this.root.dataset.tour = '';
    this.list = el('div', 'fil-list');
    this.root.append(this.list);
    stage.mount(this.root);
    const previous = stage.onStateChange;
    stage.onStateChange = (state) => {
      previous?.(state);
      this.root.classList.toggle('shown', SHOWN.has(state) && this.list.childElementCount > 0);
      // La mascotte reprend le geste de l'outil en cours en (re)passant au travail.
      if (state === 'work' && this.tool) stage.update({ tool: this.tool });
    };
  }

  private get reduced(): boolean {
    return this.stage.root.classList.contains('reduced-motion');
  }

  /** Nouvelle étape : ligne en bas du fil, les plus anciennes s'en vont. */
  push(e: Etape): void {
    if (e.tour !== this.tour) this.reset(e.tour);
    if (this.list.querySelector(`[data-id="${CSS.escape(e.id)}"]`)) return;
    // L'étape précédente s'en va, la nouvelle prend sa place.
    for (const old of this.list.querySelectorAll<HTMLElement>('.step:not(.leaving)')) this.leave(old);
    const court = { ...e, resume: courte(e) };
    const line = etapeLigne(court);
    line.title = e.resume;
    this.list.append(line);
    this.typeIn(line.querySelector('.step-text')!, court.resume);
    this.tool = e.type;
    if (this.stage.currentState === 'work') this.stage.update({ tool: e.type });
    this.root.classList.toggle('shown', SHOWN.has(this.stage.currentState));
    this.onShown?.(e);
  }

  /** Étapes affichées (texte final), de la plus ancienne à la plus récente. */
  get lines(): string[] {
    return [...this.list.querySelectorAll<HTMLElement>('.step:not(.leaving)')].map((l) => l.querySelector('.step-text')!.getAttribute('aria-label') ?? '');
  }

  private reset(tour: number): void {
    this.tour = tour;
    this.tool = undefined;
    this.root.dataset.tour = String(tour);
    for (const id of this.scrambles.values()) cancelAnimationFrame(id);
    this.scrambles.clear();
    this.list.replaceChildren();
  }

  /** Texte qui arrive brouillé puis se pose lettre par lettre (sans brouillage en mouvement réduit). */
  private typeIn(node: HTMLElement, text: string): void {
    node.setAttribute('aria-label', text);
    const letters = Array.from(text);
    if (this.reduced || letters.length === 0) {
      node.textContent = text;
      return;
    }
    // Les longues lignes se posent par blocs : une ligne est lisible en moins de 500 ms.
    const end = settleStep(letters.length);
    const speed = Math.max(1, (end * SCRAMBLE_STEP_MS) / SETTLE_MS);
    const start = performance.now();
    const paint = (k: number) => (node.textContent = scramble(letters, k).join(''));
    const loop = (now: number) => {
      const k = Math.floor(((now - start) / SCRAMBLE_STEP_MS) * speed);
      if (k < end && node.isConnected) {
        paint(k);
        this.scrambles.set(node, requestAnimationFrame(loop));
      } else {
        node.textContent = text;
        this.scrambles.delete(node);
      }
    };
    paint(0);
    this.scrambles.set(node, requestAnimationFrame(loop));
  }

  private leave(line: HTMLElement): void {
    line.classList.add('leaving');
    if (this.reduced) {
      line.remove();
      return;
    }
    window.setTimeout(() => line.remove(), LEAVE_MS);
  }
}

/** Branche le fil sur la coque (événement `activite`) ; rien hors de Tauri. */
export function followActivite(activite: Activite): void {
  const api = tauri();
  if (!api) return;
  // Délai entre l'appel d'outil reçu et la ligne dessinée, mesuré dans le journal de Kobi.
  activite.onShown = (e) =>
    requestAnimationFrame(() => void api.core.invoke('activite_affichee', { id: e.id, atMs: e.at_ms }).catch(() => {}));
  void api.event.listen<Etape>('activite', (e) => isEtape(e.payload) && activite.push(e.payload));
}
