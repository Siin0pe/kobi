// Panneaux de la section Réglages de l'app Kobi (KOB-159), dans l'ordre, et
// étapes du premier lancement (KOB-144), faites des mêmes panneaux.
//
// Ajouter un écran aux Réglages : écrire un module qui exporte un `Panneau`
// (voir panneau.ts), puis l'ajouter à la liste ci-dessous, ou l'enregistrer
// depuis son module avec `ajouterPanneau(MON_PANNEAU)` (importé par l'app).
// L'app n'a rien d'autre à changer : son menu suit cette liste.

import { AFFICHAGE } from './affichage';
import { ANIMATIONS } from './animations';
import { DEMARRAGE } from './demarrage';
import { CLAUDE, DOSSIERS, ESSAI, MICRO, MODELE, PARCOURS, RACCOURCIS } from './etapes';
import { MCP, OUTILS } from './mcp';
import type { Panneau } from './panneau';
import { SON_ORDI } from './son-ordi';
import { STOCKAGE } from './stockage';

const liste: Panneau[] = [STOCKAGE, DOSSIERS, MCP, MICRO, SON_ORDI, MODELE, CLAUDE, DEMARRAGE, ANIMATIONS, AFFICHAGE, RACCOURCIS, PARCOURS];

/** Panneaux de la section Réglages, dans l'ordre du menu. */
export const PANNEAUX: readonly Panneau[] = liste;

/** Le premier lancement, dans l'ordre du parcours UX (section 1). */
export const ETAPES: readonly Panneau[] = [CLAUDE, MICRO, SON_ORDI, MODELE, DOSSIERS, OUTILS, DEMARRAGE, ESSAI, RACCOURCIS];

export function panneau(id: string | null | undefined): Panneau | undefined {
  return PANNEAUX.find((p) => p.id === id);
}

/**
 * Enregistre un écran dans les Réglages, à la fin ou après le panneau
 * `apres`. Un identifiant déjà présent est remplacé (rechargement à chaud).
 */
export function ajouterPanneau(p: Panneau, apres?: string): void {
  const deja = liste.findIndex((x) => x.id === p.id);
  if (deja >= 0) liste.splice(deja, 1);
  const i = apres ? liste.findIndex((x) => x.id === apres) : -1;
  liste.splice(i >= 0 ? i + 1 : liste.length, 0, p);
}
