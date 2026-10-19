// Copier une réunion en sachant ce qu'on copie (KOB-224) : le menu
// « Copier » (compte rendu, transcription, notes, tout, avec la taille de
// chacun et le format), et un bouton de copie au survol de chaque partie,
// de chaque décision, action, question, note et ligne de transcription.

import { h } from '../dom';
import { caracteres, copier, FORMATS, formatCopie, lignes, poserFormatCopie, TEXTES, type Format, type Quoi } from '../copie';
import type { ReunionLue } from './donnees';
import { horodatage } from './format';

/** Une chose à copier : ce que c'est, sa taille (ou son contenu), son texte markdown. */
export interface Partie {
  quoi: Quoi;
  detail: string;
  texte: () => Promise<string>;
  /** Rien à copier (partie vide) : l'entrée est grisée. */
  vide?: boolean;
}

export type QuandCopie = (r: { message: string; apercu: string } | Error) => void;

type Segment = ReunionLue['segments'][number];

/** Une ligne de transcription, au format de l'export (stockage.rs, document_reunion). */
export function ligneMd(s: Segment): string {
  return `- \`${horodatage(s.start)}\` **${s.source === 'moi' ? 'Moi' : 'Eux'}** : ${s.text.trim()}`;
}

/**
 * Puces d'un texte markdown, dans l'ordre (une par décision, action, note…),
 * comme les rend `markdown()` (format.ts) : une `li` par ligne à puce ou
 * numérotée, quelle que soit son indentation.
 */
export function puces(md: string): string[] {
  return md
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => /^(?:[-*+]|\d+[.)])\s+/.test(l));
}

/** Le compte rendu en markdown : son fichier tel quel, ou à défaut ses parties. */
export function compteRenduMd(r: ReunionLue): string {
  const cr = r.compte_rendu;
  if (!cr) return '';
  if (cr.brut.trim()) return cr.brut.trim();
  const parties: Array<[string, string]> = [['Résumé', cr.resume], ['Décisions', cr.decisions], ['Actions', cr.actions], ['Questions ouvertes', cr.questions], ...cr.autres];
  return parties
    .filter(([, t]) => t.trim())
    .map(([titre, t]) => `## ${titre}\n\n${t.trim()}`)
    .join('\n\n');
}

/**
 * Les quatre copies d'une réunion. « Tout » est le markdown de l'export
 * (titre, compte rendu ou à défaut notes, transcription) : son détail le dit.
 */
export function partiesReunion(r: ReunionLue, tout: () => Promise<string>): Partie[] {
  const cr = compteRenduMd(r);
  const notes = r.notes.trim();
  const transcription = r.segments.map(ligneMd).join('\n');
  const contenu = [cr ? 'compte rendu' : notes ? 'notes' : null, r.segments.length ? 'transcription' : null].filter(Boolean);
  return [
    { quoi: 'compte-rendu', detail: cr ? caracteres(cr) : 'pas encore écrit', texte: async () => cr, vide: !cr },
    { quoi: 'transcription', detail: r.segments.length ? lignes(r.segments.length) : 'vide', texte: async () => transcription, vide: !r.segments.length },
    { quoi: 'notes', detail: notes ? caracteres(notes) : 'aucune', texte: async () => notes, vide: !notes },
    { quoi: 'tout', detail: `markdown : titre${contenu.length ? `, ${contenu.join(' et ')}` : ''}`, texte: tout },
  ];
}

async function lancer(p: Partie, format: Format, quand: QuandCopie): Promise<void> {
  try {
    quand(await copier(await p.texte(), p.quoi, format));
  } catch (e) {
    quand(e instanceof Error ? e : new Error(String(e)));
  }
}

/**
 * Bouton « Copier » qui ouvre un petit menu : une entrée par partie (son
 * nom, sa taille), et le format (markdown ou texte brut), retenu.
 */
export function menuCopier(parties: Partie[], quand: QuandCopie): HTMLElement {
  const bouton = h('button', { type: 'button', class: 'bouton', 'data-action': 'copier', 'aria-haspopup': 'menu', 'aria-expanded': 'false' }, 'Copier', h('span', { class: 'chevron', 'aria-hidden': 'true' }, '▾'));
  const menu = h('div', { class: 'menu-copie', role: 'menu', 'aria-label': 'Que copier ?', hidden: true });
  const entrees: HTMLButtonElement[] = [];
  for (const p of parties) {
    const e = h(
      'button',
      { type: 'button', role: 'menuitem', class: 'menu-copie-entree', 'data-quoi': p.quoi, disabled: p.vide === true, tabindex: '-1' },
      h('span', { class: 'menu-copie-nom' }, TEXTES[p.quoi].bouton),
      h('span', { class: 'menu-copie-detail' }, p.detail),
    );
    e.addEventListener('click', () => {
      fermer(true);
      void lancer(p, formatCopie(), quand);
    });
    entrees.push(e);
    menu.append(e);
  }
  const formats = h('div', { class: 'menu-copie-formats', role: 'group', 'aria-label': 'Format de la copie' }, h('span', { class: 'menu-copie-titre' }, 'Format'));
  const choix = (Object.keys(FORMATS) as Format[]).map((f) => {
    const c = h('button', { type: 'button', role: 'menuitemradio', class: 'menu-copie-format', 'data-format': f, tabindex: '-1' }, FORMATS[f]);
    c.addEventListener('click', () => {
      poserFormatCopie(f);
      marquer();
    });
    formats.append(c);
    return c;
  });
  menu.append(formats);
  const marquer = (): void => {
    const f = formatCopie();
    for (const c of choix) c.setAttribute('aria-checked', String(c.dataset.format === f));
  };
  const focusables = (): HTMLButtonElement[] => [...entrees.filter((e) => !e.disabled), ...choix];

  const dehors = (e: PointerEvent): void => {
    if (!enveloppe.contains(e.target as Node)) fermer(false);
  };
  const ouvrir = (): void => {
    marquer();
    menu.hidden = false;
    bouton.setAttribute('aria-expanded', 'true');
    focusables()[0]?.focus();
    document.addEventListener('pointerdown', dehors, true);
  };
  const fermer = (rendreFocus: boolean): void => {
    if (menu.hidden) return;
    menu.hidden = true;
    bouton.setAttribute('aria-expanded', 'false');
    document.removeEventListener('pointerdown', dehors, true);
    if (rendreFocus) bouton.focus();
  };
  bouton.addEventListener('click', () => (menu.hidden ? ouvrir() : fermer(true)));
  menu.addEventListener('keydown', (e) => {
    const liste = focusables();
    const i = liste.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') {
      e.stopPropagation();
      fermer(true);
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      liste[(i + (e.key === 'ArrowDown' ? 1 : liste.length - 1)) % liste.length]?.focus();
    } else if (e.key === 'Tab') fermer(false);
  });
  const enveloppe = h('div', { class: 'copie' }, bouton, menu);
  return enveloppe;
}

/**
 * Petit bouton de copie, montré au survol (ou au focus) de ce qu'il copie ;
 * son libellé dit quoi (« Copier cette décision »).
 */
export function boutonCopie(quoi: Quoi, texte: () => string, quand: QuandCopie): HTMLButtonElement {
  const b = h('button', { type: 'button', class: 'copie-survol', 'data-quoi': quoi, 'aria-label': TEXTES[quoi].bouton, title: TEXTES[quoi].bouton }, h('span', { class: 'copie-icone', 'aria-hidden': 'true' }), h('span', { class: 'copie-libelle' }, TEXTES[quoi].bouton));
  b.addEventListener('click', (e) => {
    // Ni la ligne cliquable autour (aller au moment de la transcription…).
    e.stopPropagation();
    void lancer({ quoi, detail: '', texte: async () => texte() }, formatCopie(), quand);
  });
  return b;
}

/** Quoi pour un élément d'une partie du compte rendu (une décision, une action…). */
export const ELEMENT: Record<string, Quoi> = { decisions: 'decision', actions: 'action', questions: 'question' };
/** Quoi pour toute une partie du compte rendu. */
export const PARTIE: Record<string, Quoi> = { resume: 'resume', decisions: 'decisions', actions: 'actions', questions: 'questions' };

/**
 * Ajoute un bouton de copie à chaque puce rendue (`li`), avec sa ligne
 * source markdown (gras, liens gardés).
 */
export function copierPuces(racine: HTMLElement, md: string, quoi: Quoi, quand: QuandCopie): void {
  const sources = puces(md);
  const items = [...racine.querySelectorAll<HTMLElement>(':scope .md > ul > li, :scope .md > ol > li')];
  items.forEach((li, i) => {
    const source = sources.length === items.length ? sources[i]! : `- ${li.textContent?.trim() ?? ''}`;
    li.classList.add('copiable');
    li.append(boutonCopie(quoi, () => source, quand));
  });
}
