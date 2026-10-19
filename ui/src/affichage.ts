// Réglages d'affichage de la conversation (KOB-169) : widget simple ou avec
// bulle détaillée, ouverture automatique de la vue agrandie, sa taille, sa
// place, la taille du texte et le délai avant qu'elle se range.
//
// Préférence d'interface gardée dans le stockage local de l'app, comme le
// réglage Animations (animations.ts) : la mascotte et l'app partagent la même
// origine, et l'événement `storage` prévient la fenêtre de Kobi dès qu'un
// réglage change dans l'app. Tout s'applique à chaud, sans redémarrer.

export type Mode = 'simple' | 'detaille';
export type Auto = 'jamais' | 'longues' | 'toujours';
export type Taille = 'petite' | 'moyenne' | 'grande';
export type Position = 'a-cote' | 'centre' | 'moitie-droite';
export type Texte = 'petit' | 'moyen' | 'grand';

export interface Affichage {
  mode: Mode;
  auto: Auto;
  taille: Taille;
  position: Position;
  texte: Texte;
  /** Secondes avant que la vue se range une fois la réponse finie ; 0 : jamais seule. */
  delai: number;
}

export const CLE_AFFICHAGE = 'kobi.affichage';

export const PAR_DEFAUT: Affichage = { mode: 'simple', auto: 'jamais', taille: 'moyenne', position: 'a-cote', texte: 'moyen', delai: 30 };

/** Valeurs permises de chaque réglage, dans l'ordre des choix. */
export const VALEURS = {
  mode: ['simple', 'detaille'],
  auto: ['jamais', 'longues', 'toujours'],
  taille: ['petite', 'moyenne', 'grande'],
  position: ['a-cote', 'centre', 'moitie-droite'],
  texte: ['petit', 'moyen', 'grand'],
  delai: [10, 30, 60, 0],
} as const;

/** Taille du panneau (px logiques) et du texte (px). */
export const TAILLES: Record<Taille, [number, number]> = { petite: [480, 420], moyenne: [640, 560], grande: [860, 720] };
export const TEXTES: Record<Texte, number> = { petit: 15, moyen: 18, grand: 22 };

function valide<K extends keyof Affichage>(cle: K, v: unknown): v is Affichage[K] {
  return (VALEURS[cle] as readonly unknown[]).includes(v);
}

export function affichage(): Affichage {
  const out = { ...PAR_DEFAUT };
  try {
    const brut = JSON.parse(window.localStorage.getItem(CLE_AFFICHAGE) ?? '{}') as Record<string, unknown>;
    for (const cle of Object.keys(PAR_DEFAUT) as Array<keyof Affichage>) {
      if (valide(cle, brut[cle])) (out as Record<string, unknown>)[cle] = brut[cle];
    }
  } catch {
    // Stockage illisible : les valeurs par défaut.
  }
  return out;
}

export function changerAffichage(patch: Partial<Affichage>): Affichage {
  const suivant = { ...affichage(), ...patch };
  try {
    window.localStorage.setItem(CLE_AFFICHAGE, JSON.stringify(suivant));
  } catch {
    // Stockage indisponible : le choix vaut pour cette fenêtre seulement.
    memoire = suivant;
  }
  ecouteurs.forEach((fn) => fn(suivant));
  return suivant;
}

let memoire: Affichage | null = null;
const ecouteurs = new Set<(a: Affichage) => void>();

/** Appelle `fn` à chaque changement, ici ou dans une autre fenêtre de Kobi. */
export function suivreAffichage(fn: (a: Affichage) => void): () => void {
  ecouteurs.add(fn);
  const surStockage = (e: StorageEvent): void => {
    if (e.key === CLE_AFFICHAGE) fn(memoire ?? affichage());
  };
  window.addEventListener('storage', surStockage);
  return () => {
    ecouteurs.delete(fn);
    window.removeEventListener('storage', surStockage);
  };
}
