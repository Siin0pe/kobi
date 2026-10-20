// Copie explicite (KOB-224) : on sait toujours ce qui part dans le
// presse-papiers. Chaque copie nomme ce qu'elle copie, son format et sa
// taille ; le retour (« Compte rendu copié · 1 240 caractères ») montre ses
// premières lignes. Partagée par le Carnet, la bulle et la vue agrandie (et
// le « Copier » du menu, KOB-217).

import { t } from './i18n';

/** Ce qu'on copie : sert au libellé du bouton et au retour. */
export type Quoi =
  | 'compte-rendu'
  | 'transcription'
  | 'notes'
  | 'tout'
  | 'conversation'
  | 'reponse'
  | 'resume'
  | 'decisions'
  | 'actions'
  | 'questions'
  | 'section'
  | 'decision'
  | 'action'
  | 'question'
  | 'ligne'
  | 'note'
  | 'selection';

export type Format = 'markdown' | 'texte';

/**
 * Textes de la copie, en un seul endroit pour l'i18n (KOB-207) : le libellé
 * du bouton et le début du retour.
 */
export const TEXTES = Object.fromEntries(
  (
    [
      'compte-rendu',
      'transcription',
      'notes',
      'tout',
      'conversation',
      'reponse',
      'resume',
      'decisions',
      'actions',
      'questions',
      'section',
      'decision',
      'action',
      'question',
      'ligne',
      'note',
      'selection',
    ] as const satisfies readonly Quoi[]
  ).map((q) => {
    // Clés `copie.<quoi>.bouton` et `copie.<quoi>.copie`, lues au rendu.
    const cle = q === 'compte-rendu' ? 'compteRendu' : q;
    return [
      q,
      {
        get bouton() {
          return t(`copie.${cle}.bouton`);
        },
        get copie() {
          return t(`copie.${cle}.copie`);
        },
      },
    ];
  }),
) as Record<Quoi, { readonly bouton: string; readonly copie: string }>;

/** Nom de chaque format, lu au rendu. */
export const FORMATS: Record<Format, string> = {
  markdown: 'markdown',
  get texte() {
    return t('copie.format.texte');
  },
};

/** « 1 240 caractères », « 1 caractère ». */
export function caracteres(texte: string): string {
  return t('copie.caracteres', { n: [...texte].length });
}

/** « 42 lignes », « 1 ligne ». */
export function lignes(n: number): string {
  return t('copie.lignes', { n });
}

/** Markdown → texte brut lisible : sans dièses, astérisques ni accents graves ; puces en « • ». */
export function texteBrut(md: string): string {
  return md
    .replace(/^#{1,6}\s+/gm, '')
    .replace(/\*\*(.+?)\*\*|__(.+?)__/g, '$1$2')
    .replace(/(^|[^*\w])\*(?!\s)([^*\n]+?)\*(?!\w)/g, '$1$2')
    .replace(/`([^`\n]+)`/g, '$1')
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '$1 ($2)')
    .replace(/^(\s*)[-*+]\s+\[[ xX]\]\s+/gm, '$1• ')
    .replace(/^(\s*)[-*+]\s+/gm, '$1• ');
}

/** Retour après copie : le message court et l'aperçu (deux premières lignes non vides). */
export function retour(quoi: Quoi, texte: string, format: Format): { message: string; apercu: string } {
  const debut = texte
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .slice(0, 2)
    .join('\n');
  return {
    message: t('copie.retour', { quoi: TEXTES[quoi].copie, taille: caracteres(texte), format: FORMATS[format] }),
    apercu: [...debut].length > 160 ? `${[...debut].slice(0, 159).join('')}…` : debut,
  };
}

/** Écrit dans le presse-papiers (repli WebKitGTK : sélection d'un champ caché). */
export async function ecrire(texte: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(texte);
  } catch {
    const zone = document.createElement('textarea');
    zone.className = 'presse-papiers';
    zone.setAttribute('aria-hidden', 'true');
    // Hors de l'écran partout, même là où la feuille du Carnet n'est pas chargée.
    zone.style.position = 'fixed';
    zone.style.left = '-9999px';
    zone.value = texte;
    document.body.append(zone);
    zone.select();
    const ok = document.execCommand('copy');
    zone.remove();
    if (!ok) throw new Error(t('copie.refusee'));
  }
}

/**
 * Copie `markdown`, converti en texte brut si `format` le demande, et
 * renvoie le retour à afficher.
 */
export async function copier(markdown: string, quoi: Quoi, format: Format = 'markdown'): Promise<{ message: string; apercu: string }> {
  const texte = format === 'texte' ? texteBrut(markdown) : markdown;
  await ecrire(texte);
  return retour(quoi, texte, format);
}

const CLE = 'kobi.copie';
/** Sans stockage (navigation privée…) : le choix vaut pour la session. */
let choisi: Format | null = null;

/** Format de copie choisi (menu « Copier » du Carnet), markdown par défaut. */
export function formatCopie(): Format {
  try {
    return localStorage.getItem(CLE) === 'texte' ? 'texte' : 'markdown';
  } catch {
    return choisi ?? 'markdown';
  }
}

export function poserFormatCopie(f: Format): void {
  choisi = f;
  try {
    localStorage.setItem(CLE, f);
  } catch {
    // Gardé en mémoire seulement.
  }
}

/** Durée du retour dans le bouton avant qu'il reprenne son libellé. */
const RETOUR_MS = 2600;

/**
 * Bouton de copie dont le retour s'affiche dans le bouton lui-même
 * (« Réponse copiée · 240 caractères »), sans notification : la bulle et la
 * vue agrandie restent sobres. Le format et l'aperçu sont dans l'infobulle.
 * `court` : sans le format dans le bouton (la bulle est étroite).
 */
export function boutonCopierDedans(quoi: Quoi, texte: () => string, classe: string, court = false): HTMLButtonElement {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = classe;
  b.dataset.quoi = quoi;
  b.textContent = TEXTES[quoi].bouton;
  b.title = TEXTES[quoi].bouton;
  let minuterie = 0;
  b.addEventListener('click', async (e) => {
    // Ni la bulle autour (qui ouvre la vue agrandie) ni un clic à côté.
    e.stopPropagation();
    window.clearTimeout(minuterie);
    const format = formatCopie();
    try {
      const r = await copier(texte(), quoi, format);
      b.textContent = court ? r.message.replace(` · ${FORMATS[format]}`, '') : r.message;
      b.title = `${r.message}\n${r.apercu}`;
      b.dataset.etat = 'copie';
    } catch {
      b.textContent = t('copie.impossible');
      b.title = t('copie.impossibleAide');
      b.dataset.etat = 'erreur';
    }
    minuterie = window.setTimeout(() => {
      b.textContent = TEXTES[quoi].bouton;
      b.title = TEXTES[quoi].bouton;
      delete b.dataset.etat;
    }, RETOUR_MS);
  });
  return b;
}
