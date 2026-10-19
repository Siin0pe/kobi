// Placement des bulles autour de Kobi (KOB-216) : une bulle (réponse, note,
// notification, carte de validation, saisie) ne mord jamais sur sa tête ; elle
// se place à côté, du côté libre de la scène, et sa pointe part vers sa bouche,
// comme une bulle de BD.
//
// Les cartes gardent leur place dessinée (CSS) tant qu'elle laisse de l'air
// devant la tête ; sinon elles glissent vers la gauche, puis se rétrécissent.
// Le calcul se fait sur la mise en page (sans les transformations : flottement,
// inclinaison, gestes de Kobi, que la marge absorbe), à l'apparition d'une
// bulle, quand elle grandit (réponse en flux) et quand la fenêtre change de
// taille ; jamais à chaque image.

/** Cartes placées : bulles, saisie, carte de validation. */
const SELECTEUR = '.bubbles .card, .ecrire, .kval';
/**
 * Air entre le bord droit d'une bulle (mise en page) et la tête : la pointe
 * (18 px), l'ombre portée (8 px) et le jeu des gestes de Kobi (le bond de
 * « idée » grandit la tête de 8 % : 12 px de chaque côté ; balancement,
 * secousse), plus le flottement de la carte.
 */
export const ECART_TETE = 48;
/** Bord gauche de la scène que les bulles ne passent pas. */
const MARGE = 16;
/** La pointe reste sur le flanc de la bulle, loin de ses coins arrondis. */
const POINTE_BORD = 20;

/** Position d'un élément dans la scène, d'après la mise en page seule. */
function dans(scene: HTMLElement, el: HTMLElement): { x: number; y: number } {
  let x = 0;
  let y = 0;
  let n: HTMLElement | null = el;
  while (n && n !== scene) {
    x += n.offsetLeft;
    y += n.offsetTop;
    n = n.offsetParent as HTMLElement | null;
  }
  return { x, y };
}

/** Change une propriété seulement si elle change (pas de mise en page pour rien). */
function poser(el: HTMLElement, prop: string, valeur: string | null): void {
  if (el.style.getPropertyValue(prop) === (valeur ?? '')) return;
  if (valeur === null) el.style.removeProperty(prop);
  else el.style.setProperty(prop, valeur);
}

/** Place une bulle à côté de la tête, la pointe vers la bouche. */
function placer(scene: HTMLElement, carte: HTMLElement, limite: number, bouche: number): void {
  // Sa place dessinée (CSS), relevée à la première vue : un placement
  // précédent ne fausse pas la mesure, et la taille ne boucle pas.
  if (!carte.dataset.place) {
    const { x } = dans(scene, carte);
    carte.dataset.place = `${x},${carte.offsetLeft},${carte.offsetWidth}`;
  }
  const [x, gaucheCss, largeur] = carte.dataset.place.split(',').map(Number) as [number, number, number];
  let gauche = x;
  let reduite: number | null = null;
  if (x + largeur > limite) {
    gauche = Math.max(MARGE, limite - largeur);
    if (gauche + largeur > limite) reduite = limite - gauche;
  }
  poser(carte, 'left', gauche === x ? null : `${gaucheCss + gauche - x}px`);
  poser(carte, 'width', reduite === null ? null : `${reduite}px`);
  const { y } = dans(scene, carte);
  // La pointe vise la bouche, sans quitter le flanc de la bulle.
  const haut = carte.offsetHeight;
  const pointe = Math.min(Math.max(bouche - y, POINTE_BORD), Math.max(POINTE_BORD, haut - POINTE_BORD));
  poser(carte, '--pointe-y', `${Math.round(pointe)}px`);
}

/**
 * Suit les bulles de la scène et les tient à l'écart de la tête de Kobi.
 * Renvoie de quoi arrêter.
 */
export function suivrePlacementBulles(scene: HTMLElement): () => void {
  const pill = scene.querySelector<HTMLElement>('.kobi .pill');
  const bouche = scene.querySelector<HTMLElement>('.kobi .mouth');
  if (!pill) return () => {};
  let prevu = 0;
  const tout = (): void => {
    prevu = 0;
    const tete = dans(scene, pill);
    const limite = tete.x - ECART_TETE;
    const yBouche = bouche ? dans(scene, bouche).y + bouche.offsetHeight / 2 : tete.y + pill.offsetHeight * 0.7;
    for (const carte of scene.querySelectorAll<HTMLElement>(SELECTEUR)) {
      if (carte.offsetParent === null) continue;
      placer(scene, carte, limite, yBouche);
    }
  };
  // Regroupé sur la prochaine image : plusieurs changements, un seul calcul.
  const plus = (): void => {
    if (!prevu) prevu = requestAnimationFrame(tout);
  };
  const tailles = new ResizeObserver(plus);
  const observer = (): void => {
    for (const carte of scene.querySelectorAll<HTMLElement>(SELECTEUR)) tailles.observe(carte);
  };
  // Une bulle apparaît ou part : on la suit. Seulement les cartes elles-mêmes
  // (pas le texte qui s'écrit ni les lettres de Kobi qui se brouillent) : leur
  // croissance passe par `tailles`.
  const arrivees = new MutationObserver(() => {
    observer();
    plus();
  });
  arrivees.observe(scene, { childList: true });
  const bulles = scene.querySelector('.bubbles');
  if (bulles) arrivees.observe(bulles, { childList: true });
  window.addEventListener('resize', plus);
  observer();
  plus();
  return () => {
    cancelAnimationFrame(prevu);
    tailles.disconnect();
    arrivees.disconnect();
    window.removeEventListener('resize', plus);
  };
}
