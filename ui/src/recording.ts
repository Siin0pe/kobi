// Enregistrement d'une réunion, affiché en permanence sur la mascotte (anneau
// rouge, « REC », durée, nombre de notes).
//
// La coque émet l'événement `enregistrement` (contrat convenu avec KOB-132) au
// début, à la pause, à la reprise, à la fin et à chaque note ; la commande
// `etat_enregistrement` renvoie le même objet au démarrage de la webview. Tant
// que la coque ne l'émet pas, l'état de session (`state` : réunion ou pause)
// sert de repli, sans durée exacte ni notes.

import type { SessionState } from './native';
import type { Stage } from './stage';

/** Charge utile de l'événement `enregistrement`. */
export interface RecordingState {
  actif: boolean;
  pause: boolean;
  /** Début de la réunion, horodatage Unix en ms. */
  debut_ms: number;
  notes: number;
  titre: string | null;
}

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> };
}

/** Applique un état d'enregistrement à la scène. */
export function applyRecording(stage: Stage, r: RecordingState): void {
  if (!r.actif) {
    stage.endMeeting();
    return;
  }
  const info = { title: r.titre ?? undefined, startedAt: r.debut_ms, paused: r.pause, notes: r.notes };
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
  void api.event.listen<RecordingState>('enregistrement', (e) => {
    explicit = true;
    applyRecording(stage, e.payload);
  });
  void api.event.listen<SessionState>('state', (e) => {
    if (!explicit) applySession(stage, e.payload);
  });
  api.core
    .invoke<RecordingState>('etat_enregistrement')
    .then((r) => {
      explicit = true;
      applyRecording(stage, r);
    })
    .catch(() => api.core.invoke<SessionState>('etat_session').then((s) => !explicit && applySession(stage, s), () => {}));
}
