// « Je m'en occupe » (KOB-152) : Kobi travaille, et son geste suit l'outil
// en cours. Il feuillette des pages (lecture), promène sa loupe (recherche),
// tape à son terminal (commande), branche un câble (MCP) ou griffonne
// (écriture, réglage). Tant qu'aucun outil n'est connu, il fouille.
//
// Tout le mouvement est en CSS (bloc `.m-work` de kobi.css) : la pause de la
// scène et « réduire les animations » le figent comme le reste de Kobi.

import type { ToolKind } from '../states';

/** Yeux de Kobi selon l'outil. */
export const WORK_EYES: Record<ToolKind | 'fouille', string> = {
  fouille: '#',
  lecture: '=',
  recherche: 'o',
  commande: '>',
  mcp: '+',
  ecriture: '/',
  reglage: '*',
};

function el(className: string, text?: string): HTMLElement {
  const node = document.createElement('div');
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export class Travail {
  readonly root: HTMLElement;

  constructor() {
    this.root = el('travail');
    this.root.dataset.outil = 'fouille';
    // Fouille : des symboles sont jetés par-dessus l'épaule.
    const fouille = el('t-fouille');
    for (const c of ['{', '#', '/', '}']) fouille.append(el('t-bit', c));
    // Lecture : trois pages qui se tournent.
    const lecture = el('t-lecture');
    for (let i = 0; i < 3; i++) lecture.append(el('t-page'));
    // Recherche : une loupe qui balaie la pilule.
    const recherche = el('t-recherche');
    recherche.append(el('t-loupe'));
    // Commande : invite qui se tape, curseur qui clignote.
    const commande = el('t-commande');
    commande.append(el('t-invite', '>'), el('t-frappe', '_ _ _'), el('t-curseur', '_'));
    // MCP : un câble entre Kobi et le service, un paquet y circule.
    const mcp = el('t-mcp');
    mcp.append(el('t-prise'), el('t-cable'), el('t-paquet'));
    // Écriture, réglage : un crayon qui griffonne.
    const ecriture = el('t-ecriture');
    ecriture.append(el('t-crayon', '/'), el('t-trait'), el('t-trait'));
    this.root.append(fouille, lecture, recherche, commande, mcp, ecriture);
  }

  /** Outil en cours (`undefined` : Kobi fouille). Renvoie vrai s'il a changé. */
  set(tool: ToolKind | undefined): boolean {
    const outil = tool === 'reglage' ? 'ecriture' : (tool ?? 'fouille');
    if (this.root.dataset.outil === outil) return false;
    this.root.dataset.outil = outil;
    // La geste repart du début : la nouvelle étape se voit.
    const groupe = this.root.querySelector<HTMLElement>(`.t-${outil}`);
    if (groupe) {
      groupe.style.animation = 'none';
      void groupe.offsetWidth;
      groupe.style.animation = '';
    }
    return true;
  }

  get outil(): string {
    return this.root.dataset.outil ?? 'fouille';
  }
}
