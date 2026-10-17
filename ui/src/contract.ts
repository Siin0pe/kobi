// Contrat d'un skin de Kobi.
//
// Un skin dessine la mascotte : il reçoit un état et des données, et n'a
// aucune logique métier. Les bulles et les cartes (question, sources, notes,
// réponse, action, erreur) sont communes à tous les skins et dessinées par la
// scène avec le thème du skin.

/** Les 8 états de la mascotte, dans l'ordre du document « mascotte et skins ». */
export const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error'] as const;

export type MascotState = (typeof STATES)[number];

export function isMascotState(value: unknown): value is MascotState {
  return typeof value === 'string' && (STATES as readonly string[]).includes(value);
}

/** Libellé court de chaque état (affichage, sélecteurs). */
export const STATE_LABELS: Record<MascotState, string> = {
  idle: 'Veille',
  listen: 'Écoute',
  think: 'Réfléchit',
  write: 'Note',
  answer: 'Répond',
  suggest: 'Propose',
  done: 'Fait',
  error: 'Oups',
};

/** Phrase lue par les lecteurs d'écran à chaque changement d'état. */
export const STATE_SPOKEN: Record<MascotState, string> = {
  idle: 'Kobi est en veille.',
  listen: 'Kobi écoute votre question.',
  think: 'Kobi réfléchit et consulte ses sources.',
  write: 'Kobi met à jour les notes.',
  answer: 'Kobi répond.',
  suggest: 'Kobi propose une action, à confirmer.',
  done: 'Action faite.',
  error: 'Kobi a rencontré un problème.',
};

/** Une source consultée pendant la réflexion (codebase, MCP, web…). */
export interface SourceItem {
  label: string;
  /** Symbole court affiché devant le libellé (`{}`, `#`, `/`…). */
  icon?: string;
  found: boolean;
}

/** Carte d'action soumise à confirmation (demande de permission d'écriture). */
export interface ActionCard {
  /** Identifiant renvoyé avec la décision de l'utilisateur. */
  id: string;
  /** Surtitre en capitales, par exemple « Je crée le ticket ? ». */
  kicker: string;
  title: string;
  meta?: string;
  status: 'pending' | 'confirmed' | 'dismissed';
  /** Textes affichés une fois l'action faite. */
  doneKicker?: string;
  doneTitle?: string;
  doneMeta?: string;
}

export interface ErrorInfo {
  /** Surtitre, par exemple « Linear ne répond pas ». */
  title: string;
  message: string;
}

/** Données affichées avec l'état. Tous les champs sont facultatifs. */
export interface MascotData {
  /** Question en cours de transcription (Écoute). */
  question?: string;
  /** Sources consultées, affichées une à une (Réfléchit). */
  sources?: SourceItem[];
  /** Notes courantes (Note). */
  notes?: string;
  /** Réponse en flux (Répond). */
  answer?: string;
  /** Étiquettes des outils cités par la réponse. */
  answerTags?: string[];
  /** Carte d'action (Propose, Fait). */
  action?: ActionCard;
  /** Erreur à expliquer en une phrase (Oups). */
  error?: ErrorInfo;
}

/** Couleurs et police d'un skin, appliquées aussi aux bulles et cartes communes. */
export interface Theme {
  /** Fond de la scène (captures, mode développement ; la fenêtre native est transparente). */
  backdrop: string;
  /** Encre : texte et fonds sombres. */
  ink: string;
  /** Papier : fond des fiches. */
  paper: string;
  /** Couleur d'accent du skin. */
  accent: string;
  /** Texte secondaire sur papier. */
  muted: string;
  /** Erreurs. */
  danger: string;
  /** Pile de polices, toutes embarquées (jamais chargées depuis Internet). */
  font: string;
  /** Couleurs de la pastille du mode discret, si le skin veut remplacer celles par défaut. */
  dots?: Partial<Record<MascotState, string>>;
}

/** Couleurs par défaut de la pastille du mode discret, une par état. */
export const DOT_COLORS: Record<MascotState, string> = {
  idle: '#6B7280',
  listen: '#22C55E',
  think: '#8B5CF6',
  write: '#0EA5E9',
  answer: '#14B8A6',
  suggest: '#F59E0B',
  done: '#16A34A',
  error: '#EF4444',
};

export interface SkinEnv {
  /** Le système demande de réduire les animations. */
  reducedMotion: boolean;
}

/**
 * Un skin. La scène appelle `mount` une fois, puis `setState` à chaque
 * changement d'état et `setData` quand seules les données changent (texte en
 * flux, source trouvée). `setPaused(true)` arrive quand la fenêtre est
 * masquée : le skin doit alors arrêter toute animation et toute boucle.
 */
export interface Skin {
  readonly id: string;
  readonly name: string;
  readonly theme: Theme;
  mount(host: HTMLElement, env: SkinEnv): void;
  setState(state: MascotState, data: Readonly<MascotData>): void;
  setData(data: Readonly<MascotData>): void;
  setPaused(paused: boolean): void;
  setReducedMotion(reduced: boolean): void;
  destroy(): void;
}

export type SkinFactory = () => Skin;
