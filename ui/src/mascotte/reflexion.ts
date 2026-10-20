// « Kobi réfléchit » (KOB-241) : trois propositions à choisir par le
// propriétaire, dans l'identité de Kobi (glyphes Recursive, vert acide).
//
// * A, engrenage : chaque œil devient une roue de glyphes qui tourne par
//   crans, les deux en sens contraires ; la tête donne un à-coup à chaque cran.
// * B, regard en l'air : yeux mi-clos en haut à droite, trois points qui se
//   tapent puis s'effacent, un « ? » de temps en temps ; tête presque immobile.
// * C, bulle de pensée : une chaîne « o O » mène à une bulle où des glyphes se
//   relaient ; la bulle gonfle et dégonfle.
//
// Tout passe par des animations CSS de transform et d'opacité : aucune boucle
// JavaScript, aucune graisse animée (KOB-200, KOB-240), et « Images par
// seconde » les met en escalier comme le reste (econome.ts). L'ancienne
// orbite (graisse animée dans ses images clés) est retirée.
//
// Choix temporaire, le temps que le propriétaire tranche :
// `localStorage['kobi.reflexion'] = 'a' | 'b' | 'c'` (défaut 'a').

export type Variante = 'a' | 'b' | 'c';
export const VARIANTES: readonly Variante[] = ['a', 'b', 'c'];
const CLE = 'kobi.reflexion';

export function variante(): Variante {
  try {
    const v = localStorage.getItem(CLE);
    if (v && (VARIANTES as readonly string[]).includes(v)) return v as Variante;
  } catch {
    // Stockage indisponible : la proposition par défaut.
  }
  return 'a';
}

function span(classe: string, texte: string): HTMLSpanElement {
  const s = document.createElement('span');
  s.className = classe;
  s.textContent = texte;
  return s;
}

/** Éléments des trois propositions, posés dans la tête ; le CSS montre celle choisie. */
export function reflexion(): HTMLElement {
  const racine = document.createElement('div');
  racine.className = 'reflexion';
  racine.setAttribute('aria-hidden', 'true');
  // A : deux roues de six glyphes, sur les yeux.
  for (const cote of ['gauche', 'droite']) {
    const roue = document.createElement('div');
    roue.className = `roue ${cote}`;
    ['{', '<', '/', '}', '>', '#'].forEach((g, i) => {
      const s = span('cran', g);
      s.style.setProperty('--i', String(i));
      roue.append(s);
    });
    racine.append(roue);
  }
  // B : trois points et un « ? », en haut à droite.
  const points = document.createElement('div');
  points.className = 'points';
  points.append(span('point', '.'), span('point', '.'), span('point', '.'), span('doute', '?'));
  racine.append(points);
  // C : chaîne et bulle de pensée.
  const pensee = document.createElement('div');
  pensee.className = 'pensee';
  const bulle = document.createElement('div');
  bulle.className = 'nuage';
  ['?', '{ }', '#', '…'].forEach((g, i) => {
    const s = span('idee', g);
    s.style.setProperty('--i', String(i));
    bulle.append(s);
  });
  pensee.append(span('maillon petit', 'o'), span('maillon', 'O'), bulle);
  racine.append(pensee);
  return racine;
}
