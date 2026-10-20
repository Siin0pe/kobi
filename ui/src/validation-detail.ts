// Carte de validation (KOB-62) : types du contrat commun des permissions
// (section 4) et rendu, partagés par la carte à côté de Kobi (courte) et par
// la section Activité de l'app Kobi (détail complet).

import { t } from './i18n';

export type Genre = 'fichier' | 'commande' | 'mcp' | 'reglage' | 'suppression';

export type Detail =
  | { diff: string }
  | { commande: string; dossier?: string | null }
  | { serveur: string; outil: string; params: unknown }
  | { cle: string; avant: unknown; apres: unknown };

/** Issue d'une carte : décidée ici, par la voix, ou à l'échéance. */
export type Statut = 'acceptee' | 'refusee' | 'expiree';

/** Carte publiée par la coque (événement `carte`, commande `cartes_validation`). */
export interface CarteValidation {
  id: string;
  type: 'validation';
  genre: Genre;
  resume: string;
  detail?: Detail | null;
  echeance_ms?: number;
  /** Absent tant que la carte attend ; posé quand la voix ou la coque a tranché. */
  statut?: Statut;
}

export function isValidation(c: unknown): c is CarteValidation {
  return typeof c === 'object' && c !== null && (c as { type?: unknown }).type === 'validation';
}

/** Glyphe de chaque genre ; son libellé est `validation.genre.<genre>`. */
export const GENRES: Record<Genre, string> = { fichier: '¶', commande: '>_', mcp: '<·>', reglage: '=', suppression: '✕' };

export function genre(c: CarteValidation): { glyph: string; kicker: string } {
  const g = c.genre in GENRES ? c.genre : 'mcp';
  return { glyph: GENRES[g], kicker: t(`validation.genre.${g}`) };
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Valeur lisible sur une ligne. */
function valeur(v: unknown): string {
  return typeof v === 'string' ? v : JSON.stringify(v);
}

/** Ce que l'action touche, sur une ligne : fichier, commande, outil ou réglage. */
export function cible(detail: Detail | null | undefined): string {
  if (!detail) return '';
  if ('diff' in detail) return /^\+\+\+ (.*)$/m.exec(detail.diff)?.[1] ?? '';
  if ('commande' in detail) return `$ ${detail.commande.split('\n')[0]}`;
  if ('serveur' in detail) return `${detail.serveur} · ${detail.outil}`;
  return `${detail.cle} : ${valeur(detail.avant)} → ${valeur(detail.apres)}`;
}

/** Détail complet : diff coloré, commande, paramètres, ou réglage avant → après. */
export function renderDetail(detail: Detail): HTMLElement {
  const box = el('div', 'kval-detail');
  if ('diff' in detail) {
    const pre = el('pre', 'kval-diff');
    for (const ligne of detail.diff.split('\n')) {
      const cls = ligne.startsWith('+++') || ligne.startsWith('---') ? 'file' : ligne.startsWith('@@') ? 'hunk' : ligne.startsWith('+') ? 'add' : ligne.startsWith('-') ? 'del' : 'ctx';
      pre.append(el('span', cls, ligne), '\n');
    }
    box.append(pre);
  } else if ('commande' in detail) {
    const pre = el('pre', 'kval-cmd');
    pre.append(el('span', 'prompt', '$ '), detail.commande);
    box.append(pre);
    if (detail.dossier) box.append(el('div', 'kval-where', t('validation.dans', { dossier: detail.dossier })));
  } else if ('serveur' in detail) {
    // Serveur et outil sont déjà sur la ligne de la cible : les paramètres seuls.
    box.append(el('pre', 'kval-params', JSON.stringify(detail.params, null, 2)));
  } else {
    const row = el('div', 'kval-setting');
    row.append(el('code', 'cle', detail.cle), el('span', 'avant', valeur(detail.avant)), el('span', 'fleche', '→'), el('span', 'apres', valeur(detail.apres)));
    box.append(row);
  }
  return box;
}
