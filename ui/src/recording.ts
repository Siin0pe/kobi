// Enregistrement d'une réunion, affiché en permanence sur la mascotte (anneau
// rouge, « REC », durée, nombre de notes).
//
// La coque émet l'événement `enregistrement` (contrat convenu avec KOB-132) au
// début, à la pause, à la reprise, à la fin et à chaque note ; la commande
// `etat_enregistrement` renvoie le même objet au démarrage de la webview. Tant
// que la coque ne l'émet pas, l'état de session (`state` : réunion ou pause)
// sert de repli, sans durée exacte ni notes.

import type { SessionState } from './native';
import type { MeetingInfo } from './states';
import type { Stage } from './stage';

/** Charge utile de l'événement `enregistrement`. */
export interface RecordingState {
  actif: boolean;
  pause: boolean;
  /** Début de la réunion, horodatage Unix en ms. */
  debut_ms: number;
  notes: number;
  titre: string | null;
  /** Dernière note prise (facultatif), tapée dans la fiche de Kobi. */
  derniere_note?: string | null;
  /** Réunion terminée, fin encore annulable (KOB-245), avec `actif` faux. */
  annulable?: boolean;
}

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> };
}

/**
 * Dernier retard de la transcription signalé par la coque (KOB-250) : gardé
 * même hors réunion, car il peut arriver juste avant que la réunion démarre.
 */
let retard: MeetingInfo['late'];

/** Applique un état d'enregistrement à la scène. */
export function applyRecording(stage: Stage, r: RecordingState): void {
  if (!r.actif) {
    if (stage.currentMeeting) stage.endMeeting({ undoable: Boolean(r.annulable) });
    else if (!r.annulable) stage.closeUndo();
    return;
  }
  const info = {
    title: r.titre ?? undefined,
    startedAt: r.debut_ms,
    paused: r.pause,
    notes: r.notes,
    late: retard,
    lastNote: r.derniere_note ?? undefined,
  };
  if (stage.currentMeeting) stage.updateMeeting(info);
  else stage.startMeeting(info, { consent: false });
}

/** Repli sur l'état de session : réunion ou pause = enregistrement. */
export function applySession(stage: Stage, s: Pick<SessionState, 'etat'>): void {
  const recording = s.etat === 'reunion' || s.etat === 'pause';
  if (!recording) {
    stage.endMeeting();
    return;
  }
  const paused = s.etat === 'pause';
  if (stage.currentMeeting) stage.updateMeeting({ paused });
  else stage.startMeeting({ startedAt: Date.now(), paused }, { consent: false });
}

/** Suit l'enregistrement dans l'application (rien hors de Tauri). */
export function followRecording(stage: Stage): void {
  const api = (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
  if (!api) return;
  // Dès que la coque parle d'enregistrement, l'état de session ne sert plus.
  let explicit = false;
  // Son de l'ordinateur capté (KOB-164) : indicateur discret sur REC.
  let sonOrdi = false;
  const marquer = () => stage.updateMeeting({ computerAudio: sonOrdi });
  void api.event.listen<boolean>('son_ordi', (e) => {
    sonOrdi = e.payload;
    marquer();
  });
  api.core.invoke<{ capte: boolean } | null>('son_ordi').then(
    (s) => {
      // Coque d'une autre version (ou de test) : pas de réponse, rien de capté.
      sonOrdi = Boolean(s?.capte);
      marquer();
    },
    () => {},
  );
  // « Annuler » (clic ou Ctrl+Z) : la même réunion reprend (KOB-245).
  const annuler = (): void => {
    void api.core.invoke('reprendre').catch(() => {});
  };
  stage.onUndoMeeting = annuler;
  document.addEventListener('keydown', (e) => {
    if (e.key.toLowerCase() !== 'z' || !(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
    if (!stage.canUndoMeeting) return;
    e.preventDefault();
    annuler();
  });
  // Retard de la transcription (KOB-250) : affiché sur REC, avec le raccourci.
  void api.event.listen<{ secondes: number; raccourci: string } | null>('transcription_retard', (e) => {
    const r = e.payload;
    retard = r ? { seconds: r.secondes, shortcut: r.raccourci } : undefined;
    stage.updateMeeting({ late: retard });
  });
  void api.event.listen<RecordingState>('enregistrement', (e) => {
    explicit = true;
    applyRecording(stage, e.payload);
    marquer();
  });
  void api.event.listen<SessionState>('state', (e) => {
    if (!explicit) applySession(stage, e.payload);
  });
  api.core
    .invoke<RecordingState>('etat_enregistrement')
    .then((r) => {
      explicit = true;
      applyRecording(stage, r);
      marquer();
    })
    .catch(() => api.core.invoke<SessionState>('etat_session').then((s) => !explicit && applySession(stage, s), () => {}));
}
