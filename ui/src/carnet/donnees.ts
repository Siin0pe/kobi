// Ce que renvoient les commandes du Carnet (crates/kobi-app/src/carnet.rs,
// types de kobi_session::stockage). Aucun accès au disque hors de ces
// commandes.

import { invoke } from '../ipc';

export type Genre = 'reunion' | 'conversation';

export interface ResumeReunion {
  nom: string;
  date: string | null;
  titre: string | null;
  debut_ms: number | null;
  fin_ms: number | null;
  en_cours: boolean;
  duree_s: number;
  a_compte_rendu: boolean;
  apercu: string;
}

export interface ResumeConversation {
  id: string;
  date: string;
  debut_ms: number | null;
  fin_ms: number | null;
  tours: number;
  apercu: string;
}

export interface Rappel {
  id: string;
  texte: string;
  echeance_ms: number | null;
  cree_ms: number;
  fait: boolean;
  source: string | null;
}

export interface Index {
  racine: string;
  reunions: ResumeReunion[];
  conversations: ResumeConversation[];
  rappels: Rappel[];
}

export interface Segment {
  start: number;
  end: number;
  source: 'moi' | 'eux';
  text: string;
  is_final: boolean;
}

export interface CompteRendu {
  resume: string;
  decisions: string;
  actions: string;
  questions: string;
  autres: Array<[string, string]>;
  brut: string;
}

export interface ReunionLue {
  resume: ResumeReunion;
  dossier: string;
  segments: Segment[];
  notes: string;
  compte_rendu: CompteRendu | null;
}

export interface Tour {
  at_ms: number;
  qui: 'moi' | 'kobi';
  texte: string;
  voix: boolean;
}

export interface ConversationLue {
  resume: ResumeConversation;
  chemin: string;
  tours: Tour[];
}

export type Trouve =
  | {
      type: 'reunion';
      nom: string;
      titre: string | null;
      partie: 'transcription' | 'notes' | 'compte-rendu';
      start: number | null;
      extrait: string;
    }
  | { type: 'conversation'; id: string; at_ms: number; extrait: string };

export const carnet = {
  index: () => invoke<Index>('carnet_index'),
  reunion: (nom: string) => invoke<ReunionLue>('carnet_reunion', { nom }),
  conversation: (id: string) => invoke<ConversationLue>('carnet_conversation', { id }),
  chercher: (requete: string) => invoke<Trouve[]>('carnet_chercher', { requete }),
  markdown: (genre: Genre, id: string) => invoke<string>('carnet_markdown', { genre, id }),
  exporter: (genre: Genre, id: string) => invoke<string | null>('carnet_exporter', { genre, id }),
  ouvrirDossier: (genre?: Genre, id?: string) => invoke<void>('carnet_ouvrir_dossier', genre && id ? { genre, id } : {}),
  supprimer: (genre: Genre, id: string) => invoke<void>('carnet_supprimer', { genre, id }),
  rappelFait: (id: string) => invoke<boolean>('carnet_rappel_fait', { id }),
};
