// Traduction de l'interface (KOB-207) : `t('cle', { n: 3 })` donne le texte
// dans la langue choisie (`ui.language` de la config), le français en repli.
//
// Un fichier par langue (`fr.json`, `en.json`), mêmes clés partout ; une clé
// qui manque dans une langue (ou une variable qui diffère) fait échouer le
// test Rust `i18n::tests::memes_cles_et_variables_dans_chaque_langue`.
//   - Variables : `{nom}` dans le texte, remplacée par `vars.nom`.
//   - Pluriels : une valeur objet `{ "one": "…", "other": "…" }` (catégories
//     de `Intl.PluralRules`), choisie selon `vars.n`.
//
// Ajouter une langue : copier `fr.json` en `xx.json`, le traduire, l'ajouter
// à `LANGUES` ci-dessous (voir le README, « Ajouter une langue »).

import { disponible, ecouter, invoke } from '../ipc';
import en from './en.json';
import fr from './fr.json';

type Texte = string | Partial<Record<Intl.LDMLPluralRule, string>>;
type Catalogue = Record<string, Texte>;

/** Langues proposées, dans l'ordre du choix, chacune nommée dans sa langue. */
export const LANGUES = [
  { code: 'fr', nom: 'Français', catalogue: fr as Catalogue },
  { code: 'en', nom: 'English', catalogue: en as Catalogue },
] as const;

export type Langue = (typeof LANGUES)[number]['code'];

const REPLI: Langue = 'fr';
let courante: Langue = REPLI;
const abonnes = new Set<(l: Langue) => void>();

export function estLangue(x: unknown): x is Langue {
  return LANGUES.some((l) => l.code === x);
}

/** La langue du système si Kobi la parle, sinon le français. */
export function langueDuSysteme(): Langue {
  for (const tag of navigator.languages ?? [navigator.language]) {
    const code = tag.toLowerCase().split('-')[0];
    if (estLangue(code)) return code;
  }
  return REPLI;
}

export function langue(): Langue {
  return courante;
}

function catalogue(l: Langue): Catalogue {
  return LANGUES.find((x) => x.code === l)!.catalogue;
}

/** Le texte de `cle` dans la langue courante ; la clé elle-même si elle n'existe nulle part. */
export function t(cle: string, vars: Record<string, string | number> = {}): string {
  const brut = catalogue(courante)[cle] ?? catalogue(REPLI)[cle];
  if (brut === undefined) return cle;
  let texte: string;
  if (typeof brut === 'string') texte = brut;
  else {
    const n = Number(vars.n ?? 0);
    const cat = new Intl.PluralRules(courante).select(n);
    texte = brut[cat] ?? brut.other ?? '';
  }
  return texte.replace(/\{(\w+)\}/g, (m, nom: string) => (vars[nom] !== undefined ? formaterNombre(vars[nom]) : m));
}

function formaterNombre(v: string | number): string {
  return typeof v === 'number' ? new Intl.NumberFormat(courante).format(v) : v;
}

/** Date et heure au format de la langue (mêmes options que `Intl.DateTimeFormat`). */
export function date(d: Date | number, options: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(courante, options).format(d);
}

/** Change la langue de cette fenêtre et prévient ses abonnés (sans rien enregistrer). */
export function appliquerLangue(l: Langue): void {
  if (l === courante) return;
  courante = l;
  document.documentElement.lang = l;
  for (const f of abonnes) f(l);
}

/** Appelé à chaque changement de langue ; retourne de quoi se désabonner. */
export function surLangue(f: (l: Langue) => void): () => void {
  abonnes.add(f);
  return () => abonnes.delete(f);
}

/**
 * Au démarrage d'une fenêtre : lit `ui.language` dans Kobi (la langue du
 * système hors de Kobi) et suit ses changements, faits depuis n'importe
 * quelle fenêtre ou par `kobi config set ui.language en`.
 */
export async function initLangue(): Promise<Langue> {
  document.documentElement.lang = courante;
  // `null` : la langue du système.
  ecouter<string | null>('langue', (l) => appliquerLangue(estLangue(l) ? l : langueDuSysteme()));
  const l = await reglee().catch(() => null);
  appliquerLangue(l ?? langueDuSysteme());
  return courante;
}

/** La langue réglée dans Kobi, `null` pour celle du système. */
export async function reglee(): Promise<Langue | null> {
  const l = await invoke<string | null>('langue_ui');
  return estLangue(l) ? l : null;
}

/**
 * Enregistre la langue dans la config (`null` : celle du système) ; Kobi
 * prévient toutes les fenêtres. Hors de Kobi, seule cette fenêtre change.
 */
export async function choisirLangue(l: Langue | null): Promise<Langue> {
  const retenue = disponible() ? await invoke<string | null>('regler_langue_ui', { langue: l }) : l;
  appliquerLangue(estLangue(retenue) ? retenue : langueDuSysteme());
  return courante;
}
