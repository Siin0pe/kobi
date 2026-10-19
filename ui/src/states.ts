// États de Kobi et données affichées avec eux.
//
// La mascotte reçoit un état et des données, sans logique métier. Les bulles
// et les cartes (question, sources, notes, réponse, action, erreur) sont
// dessinées par la scène.

/**
 * Les états de la mascotte : les 8 du document « mascotte et skins », puis
 * « je m'en occupe » (KOB-152), tant que Claude travaille avec ses outils, et
 * `compose`, la rédaction du compte rendu après une réunion (KOB-166),
 * distincte de la prise de notes (`write`, seulement pendant l'enregistrement).
 */
export const STATES = ['idle', 'listen', 'think', 'write', 'answer', 'suggest', 'done', 'error', 'work', 'compose'] as const;

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
  work: 'Je m’en occupe',
  compose: 'Rédige',
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
  work: 'Kobi s’en occupe et montre chaque étape.',
  compose: 'Réunion terminée. Kobi rédige le compte rendu.',
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

/**
 * Réunion en cours : l'indicateur REC reste visible tant qu'elle dure, quel que
 * soit l'état de la mascotte (y compris en mode discret).
 */
export interface MeetingInfo {
  /** Titre facultatif, affiché à côté de REC. */
  title?: string;
  /** Début de la réunion (horodatage en ms, `Date.now()`), pour la durée affichée. */
  startedAt: number;
  /** Transcription en pause (« Kobi, pause » ou un clic). */
  paused?: boolean;
  /**
   * Arrêt annulable (KOB-165, KOB-174) : l'enregistrement est arrêté, mais
   * « reprends » le relance pendant 30 s ; rien n'est encore rédigé.
   */
  stopping?: boolean;
  /** Nombre de notes prises depuis le début de la réunion. */
  notes?: number;
  /** Texte de la dernière note prise, tapé dans la fiche pendant l'enregistrement. */
  lastNote?: string;
}

export type ConsentLang = 'fr' | 'en';

/** Messages prêts à coller pour prévenir les participants qu'ils sont transcrits. */
export const CONSENT_MESSAGES: Record<ConsentLang, string> = {
  fr: 'Pour information : j’utilise Kobi, un assistant qui transcrit cette réunion en local sur mon ordinateur pour m’aider à prendre des notes. L’audio ne quitte pas ma machine. Dites-le-moi si vous préférez que je l’arrête.',
  en: 'Heads-up: I’m using Kobi, an assistant that transcribes this meeting locally on my computer to help me take notes. The audio never leaves my machine. Let me know if you’d rather I turn it off.',
};

/** Famille de l'outil en cours (événement `activite` de la coque, KOB-152). */
export type ToolKind = 'lecture' | 'recherche' | 'commande' | 'mcp' | 'ecriture' | 'reglage';

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
  /** Outil en cours (Je m'en occupe) : l'animation de Kobi le suit. */
  tool?: ToolKind;
}

/** Couleurs de la pastille du mode discret, une par état. */
export const DOT_COLORS: Record<MascotState, string> = {
  idle: '#0B0B0B',
  listen: '#D7FF3A',
  think: '#8B5CF6',
  write: '#0EA5E9',
  answer: '#14B8A6',
  suggest: '#F59E0B',
  done: '#16A34A',
  error: '#EF4444',
  work: '#6366F1',
  compose: '#64748B',
};
