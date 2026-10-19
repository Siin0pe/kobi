// Place du menu du clic droit sur l'écran (KOB-202), comme un menu contextuel
// Windows : son coin haut-gauche sous le pointeur ; s'il déborde de la zone
// de travail de l'écran (bord droit, bas, barre des tâches), il se retourne
// vers la gauche ou vers le haut pour rester entier. Tout est en pixels
// physiques de l'écran : la coque donne le pointeur et la zone de travail,
// le menu sa taille multipliée par l'échelle de l'écran.

export interface Point {
  x: number;
  y: number;
}

/** Zone de travail de l'écran (sans la barre des tâches). */
export interface Zone {
  x: number;
  y: number;
  largeur: number;
  hauteur: number;
}

export interface Place {
  /** Coin haut-gauche du menu. */
  x: number;
  y: number;
  /** Retourné : le pointeur est au coin haut-droit (et/ou bas) du menu. */
  versGauche: boolean;
  versHaut: boolean;
}

export function placer(pointeur: Point, largeur: number, hauteur: number, zone: Zone): Place {
  const droite = zone.x + zone.largeur;
  const bas = zone.y + zone.hauteur;
  const versGauche = pointeur.x + largeur > droite;
  const versHaut = pointeur.y + hauteur > bas;
  const x = versGauche ? pointeur.x - largeur : pointeur.x;
  const y = versHaut ? pointeur.y - hauteur : pointeur.y;
  // Écran plus petit que le menu, ou pointeur hors de la zone (sur la barre
  // des tâches) : le menu reste dans la zone.
  return {
    x: Math.round(Math.max(zone.x, Math.min(x, droite - largeur))),
    y: Math.round(Math.max(zone.y, Math.min(y, bas - hauteur))),
    versGauche,
    versHaut,
  };
}
