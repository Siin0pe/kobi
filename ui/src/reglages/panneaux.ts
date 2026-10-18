// Liste des panneaux de réglages, dans l'ordre de la fenêtre Réglages.
// Démarrage avec Windows (KOB-143), dossiers de code (KOB-144), mode de
// réponse (KOB-142) viendront s'ajouter ici.

import type { Panneau } from './panneau';
import { STOCKAGE } from './stockage';

export const PANNEAUX: readonly Panneau[] = [STOCKAGE];

export function panneau(id: string | null | undefined): Panneau | undefined {
  return PANNEAUX.find((p) => p.id === id);
}
