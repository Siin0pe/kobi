// Cartes de confirmation et interruption, branchées sur la coque.
//
// Événement `carte` (CarteUi : id, kicker, title, meta, status) : en attente,
// Kobi propose ; confirmée, c'est fait ; refusée, il repasse au calme. Le clic
// renvoie `decider_carte { id, accepte }`. Échap interrompt la réponse en cours
// (`interrompre`) : la frappe s'arrête tout de suite, sans attendre la coque.

import type { Conversation } from './conversation';
import { tauri } from './native';
import type { Stage } from './stage';
import type { ActionCard } from './states';
import { isValidation, showValidation } from './validation';

/** Carte telle que la publie la coque. */
export interface CarteUi {
  id: string;
  kicker: string;
  title: string;
  meta: string | null;
  status: ActionCard['status'];
}

/** Cartes encore en attente de chaque scène, dans l'ordre d'arrivée. */
const enAttente = new WeakMap<Stage, Map<string, ActionCard>>();

/**
 * Affiche une carte publiée par la coque. Plusieurs cartes peuvent attendre
 * ensemble (une de la conversation, une des notes en continu) : la plus
 * récente est montrée, et quand elle se ferme, la suivante encore en attente
 * reprend sa place, au lieu de disparaître alors qu'elle attend (KOB-343).
 */
export function applyCarte(stage: Stage, c: CarteUi): void {
  if (isValidation(c)) return showValidation(stage, c);
  const action: ActionCard = { id: c.id, kicker: c.kicker, title: c.title, meta: c.meta ?? undefined, status: c.status };
  let file = enAttente.get(stage);
  if (!file) enAttente.set(stage, (file = new Map()));
  if (c.status === 'pending') {
    file.delete(c.id);
    file.set(c.id, action);
    stage.setState('suggest', { action });
    return;
  }
  file.delete(c.id);
  const suivante = [...file.values()].pop();
  if (suivante) stage.setState('suggest', { action: suivante });
  else if (c.status === 'confirmed') stage.setState('done', { action });
  else stage.setState('idle');
}

/** Échap : l'historique ouvert se referme ; sinon la réponse s'arrête net. */
export function enableInterrupt(stage: Stage, conversation: Conversation, onInterrupt: () => void): void {
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (conversation.historyOpen) {
      conversation.closeHistory();
      return;
    }
    if (stage.currentState !== 'answer' && stage.currentState !== 'think' && stage.currentState !== 'listen') return;
    stage.interrupt();
    onInterrupt();
  });
}

/** Branche cartes et interruption sur la coque (rien hors de Tauri). */
export function followCartes(stage: Stage): () => void {
  const api = tauri();
  if (!api) return () => {};
  void api.event.listen<CarteUi>('carte', (e) => applyCarte(stage, e.payload));
  stage.onAction((d) => void api.core.invoke('decider_carte', { id: d.id, accepte: d.decision === 'confirm' }).catch(() => {}));
  return () => void api.core.invoke('interrompre').catch(() => {});
}
