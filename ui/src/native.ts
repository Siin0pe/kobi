// Intégration à la coque Tauri (API globale `window.__TAURI__`, activée par
// `withGlobalTauri`). Hors de Tauri (navigateur, tests), rien ne se passe.
//
// L'interface ne contient aucune logique métier : elle envoie des commandes à
// la session (démarrer, pause, reprendre, terminer) et affiche l'état que la
// session lui renvoie par l'événement `state`.

import { followRecording } from './recording';
import { isMascotState } from './states';
import type { Stage } from './stage';

export interface TauriGlobal {
  window: { getCurrentWindow(): { startDragging(): Promise<void> } };
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> };
}

export function tauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

export function isNative(): boolean {
  return tauri() !== undefined;
}

/** Glisser la mascotte déplace la fenêtre ; les cartes et leurs boutons restent cliquables. */
export function enableWindowDrag(root: HTMLElement): void {
  const api = tauri();
  if (!api) return;
  root.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    const target = e.target as Element | null;
    if (target?.closest('button, a, input, textarea, select, .card')) return;
    e.preventDefault();
    void api.window.getCurrentWindow().startDragging();
  });
}

/** État de la session, tel que l'envoie `kobi-app` (événement `state`). */
export interface SessionState {
  /** `veille`, `assistant` (échange hors réunion), `reunion`, `pause` ou `fin`. */
  etat: 'veille' | 'assistant' | 'reunion' | 'pause' | 'fin';
  /** Activité en réunion ou en échange (`calme`, `ecoute`…), `null` sinon. */
  activite: string | null;
  /** État de la mascotte (`idle`, `listen`…). */
  mascotte: string;
  /** La transcription tourne (indicateur REC). */
  transcrit: boolean;
}

/** Segment de transcription (événement `segment`). */
export interface Segment {
  start: number;
  end: number;
  source: 'moi' | 'eux';
  text: string;
  is_final: boolean;
}

export const COMMANDS = ['demarrer', 'pause', 'reprendre', 'terminer'] as const;
export type Command = (typeof COMMANDS)[number];

/** Envoie une commande à la session ; `undefined` hors de Tauri. Un refus rejette la promesse. */
export function command(name: Command): Promise<SessionState | undefined> {
  const api = tauri();
  return api ? api.core.invoke<SessionState>(name) : Promise.resolve(undefined);
}

/**
 * Affiche l'état de la session : la mascotte suit l'événement `state`, la
 * racine porte `data-session` et `data-rec`. Les segments sont relayés en
 * événement DOM `kobi:segment` (panneau de transcription, KOB-50).
 */
export function followSession(stage: Stage): void {
  const api = tauri();
  if (!api) return;
  const show = (s: SessionState) => {
    stage.root.dataset.session = s.etat;
    stage.root.dataset.rec = String(s.transcrit);
    if (isMascotState(s.mascotte) && s.mascotte !== stage.root.dataset.state) stage.setState(s.mascotte);
  };
  void api.event.listen<SessionState>('state', (e) => show(e.payload));
  void api.event.listen<Segment>('segment', (e) =>
    window.dispatchEvent(new CustomEvent<Segment>('kobi:segment', { detail: e.payload })),
  );
  // Interpellation de bout en bout (KOB-67) : question transcrite en direct,
  // réponse de Claude en flux, erreur à afficher.
  void api.event.listen<string>('question', (e) => stage.update({ question: e.payload }));
  void api.event.listen<string>('answer', (e) => stage.update({ answer: e.payload }));
  void api.event.listen<{ title: string; message: string }>('erreur', (e) => stage.update({ error: e.payload }));
  // Volume du micro (0 à 1) pendant l'écoute, s'il est émis par la coque.
  void api.event.listen<number>('niveau', (e) => typeof e.payload === 'number' && stage.setLevel(e.payload));
  void api.core.invoke<SessionState>('etat_session').then(show);
  followRecording(stage);
}
