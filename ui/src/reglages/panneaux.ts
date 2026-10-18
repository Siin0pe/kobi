// Liste des panneaux de réglages, dans l'ordre de la fenêtre Réglages.
// Dossiers de code (KOB-144) et mode de réponse (KOB-142) viendront
// s'ajouter ici.

import { DEMARRAGE } from './demarrage';
import type { Panneau } from './panneau';
import { STOCKAGE } from './stockage';

export const PANNEAUX: readonly Panneau[] = [STOCKAGE, DEMARRAGE];

export function panneau(id: string | null | undefined): Panneau | undefined {
  return PANNEAUX.find((p) => p.id === id);
}
