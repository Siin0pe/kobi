// Effets des bulles, cartes et notifications (KOB-150) : tout part de Kobi et
// y revient. Entrée distribuée depuis la pilule (rebond, légère rotation),
// sortie froissée vers elle, lettres qui volent de Kobi vers la fiche de notes.
//
// Tous passent par `el.animate()` (animations Web), jamais par une animation
// CSS finie : au repos, le métronome de la scène (econome.ts) les tient à son
// rythme, alors qu'une animation CSS finie tournerait à 60 images/s. Toutes
// s'effacent devant « réduire les animations » (classe `reduced-motion` posée
// par Kobi selon son réglage, jamais le média du système).

import './styles/effets.css';

/** Centre de la pilule de Kobi dans la scène (820 × 560). */
export const KOBI = { x: 578, y: 258 } as const;
/**
 * Bord de sa bouche, côté bulles (KOB-216) : les cartes en sortent et y
 * reviennent, comme une parole, sans passer sur son visage.
 */
export const BOUCHE = { x: 440, y: 300 } as const;

/** « Réduire les animations » est actif pour cet élément. */
export function reduced(node: Element): boolean {
  return node.closest('.reduced-motion') !== null;
}

/** Centre d'un élément dans le repère de la scène (insensible à sa mise à l'échelle). */
function center(node: HTMLElement): { x: number; y: number } {
  let x = node.offsetWidth / 2;
  let y = node.offsetHeight / 2;
  for (let n: HTMLElement | null = node; n && !n.classList.contains('scene'); n = n.offsetParent as HTMLElement | null) {
    x += n.offsetLeft;
    y += n.offsetTop;
  }
  return { x, y };
}

/** Écart de l'élément à la bouche de Kobi, en px de scène. */
function fromKobi(node: HTMLElement): { dx: number; dy: number } {
  const c = center(node);
  return { dx: BOUCHE.x - c.x, dy: BOUCHE.y - c.y };
}

/**
 * Entrée distribuée : la carte sort de Kobi, petite et tournée, file vers sa
 * place, la dépasse un peu et s'y pose. Additive : la rotation propre de la
 * carte (CSS) reste intacte.
 */
export function dealFromKobi(node: HTMLElement, delay = 0): Animation | null {
  if (reduced(node)) return null;
  const { dx, dy } = fromKobi(node);
  const turn = dx > 0 ? -14 : 14;
  return node.animate(
    [
      { transform: `translate(${dx}px, ${dy}px) scale(.18) rotate(${turn}deg)`, opacity: 0 },
      { transform: `translate(${dx * 0.25}px, ${dy * 0.25 - 18}px) scale(.8) rotate(${turn / 3}deg)`, opacity: 1, offset: 0.45 },
      { transform: 'translate(0, 4px) scale(1.04, .97) rotate(-1deg)', offset: 0.72 },
      { transform: 'translate(0, -2px) scale(.99, 1.01)', offset: 0.87 },
      { transform: 'none', opacity: 1 },
    ],
    { duration: 560, delay, easing: 'cubic-bezier(.2,.75,.3,1)', fill: 'backwards', composite: 'add' },
  );
}

/** Durée de la sortie vers Kobi. */
export const LEAVE_MS = 340;

/**
 * Sortie : la carte se froisse et retourne dans Kobi, puis `done` (retrait du
 * DOM). Jamais de disparition sèche, sauf avec « réduire les animations ».
 */
export function leaveToKobi(node: HTMLElement, done: () => void): void {
  if (reduced(node)) {
    done();
    return;
  }
  const { dx, dy } = fromKobi(node);
  const anim = node.animate(
    [
      { transform: 'none', opacity: 1 },
      { transform: 'translate(0, -8px) scale(1.05, .9) rotate(2deg)', opacity: 1, offset: 0.25 },
      { transform: `translate(${dx * 0.85}px, ${dy * 0.85}px) scale(.12) rotate(${dx > 0 ? 28 : -28}deg)`, opacity: 0 },
    ],
    { duration: LEAVE_MS, easing: 'cubic-bezier(.5,0,.75,.2)', fill: 'forwards', composite: 'add' },
  );
  anim.onfinish = done;
  anim.oncancel = done;
}

/**
 * Prise de notes : quelques lettres de la note s'envolent de Kobi vers
 * `target` en arc, une par une, puis disparaissent dans la fiche.
 */
export function flyLetters(layer: HTMLElement, target: HTMLElement, text: string, max = 7): void {
  if (reduced(layer)) return;
  const letters = Array.from(text.replace(/\s+/g, '')).slice(-max);
  if (letters.length === 0) return;
  const to = center(target);
  letters.forEach((ch, i) => {
    const span = document.createElement('span');
    span.className = 'lettre-volante';
    span.textContent = ch;
    span.setAttribute('aria-hidden', 'true');
    span.style.left = `${KOBI.x}px`;
    span.style.top = `${KOBI.y}px`;
    layer.append(span);
    const dx = to.x - KOBI.x + (i - letters.length / 2) * 9;
    const dy = to.y - KOBI.y;
    const anim = span.animate(
      [
        { transform: 'translate(0, 0) scale(.4) rotate(0deg)', opacity: 0 },
        { transform: `translate(${dx * 0.45}px, ${dy * 0.45 - 70}px) scale(1.25) rotate(${(i % 2 ? 1 : -1) * 160}deg)`, opacity: 1, offset: 0.5 },
        { transform: `translate(${dx}px, ${dy}px) scale(.5) rotate(${(i % 2 ? 1 : -1) * 340}deg)`, opacity: 0 },
      ],
      { duration: 760, delay: i * 70, easing: 'cubic-bezier(.3,.6,.4,1)', fill: 'backwards' },
    );
    anim.onfinish = () => span.remove();
    anim.oncancel = () => span.remove();
  });
}
