// Liste des panneaux de réglages, dans l'ordre de la fenêtre Réglages, et
// les étapes du premier lancement (KOB-144), faites des mêmes panneaux.
// Mode de réponse (KOB-142) viendra s'ajouter ici.

import { DEMARRAGE } from './demarrage';
import { CLAUDE, DOSSIERS, ESSAI, MICRO, MODELE, PARCOURS, RACCOURCIS } from './etapes';
import type { Panneau } from './panneau';
import { STOCKAGE } from './stockage';

export const PANNEAUX: readonly Panneau[] = [STOCKAGE, DOSSIERS, MICRO, MODELE, CLAUDE, DEMARRAGE, RACCOURCIS, PARCOURS];

/** Le premier lancement, dans l'ordre du parcours UX (section 1). */
export const ETAPES: readonly Panneau[] = [CLAUDE, MICRO, MODELE, DOSSIERS, DEMARRAGE, ESSAI, RACCOURCIS];

export function panneau(id: string | null | undefined): Panneau | undefined {
  return PANNEAUX.find((p) => p.id === id);
}
