// Une seule boucle requestAnimationFrame pour toute l'interface : brouillage
// des caractères, frappe des bulles, volume du micro, suivi du curseur…
// Chaque abonné est appelé à chaque image avec l'heure de l'image et renvoie
// `true` pour être rappelé à l'image suivante. Sans abonné, la boucle s'arrête.

type Tick = (now: number) => boolean;

const subscribers = new Set<Tick>();
let frame = 0;

/** Intervalle imposé entre deux images (repos économe), 0 = chaque image. */
let interval = 0;
let timer = 0;

function request(): void {
  if (interval > 0) timer = window.setTimeout(() => (frame = requestAnimationFrame(loop)), interval);
  else frame = requestAnimationFrame(loop);
}

/**
 * Repos économe : la boucle ne tourne plus qu'une image toutes les `ms`
 * (le métronome de la scène, econome.ts) ; 0 la rend à chaque image.
 */
export function setFrameInterval(ms: number): void {
  interval = ms;
}

function loop(now: number): void {
  frame = 0;
  timer = 0;
  for (const fn of [...subscribers]) {
    if (!subscribers.has(fn)) continue;
    let again = false;
    try {
      again = fn(now);
    } finally {
      if (!again) subscribers.delete(fn);
    }
  }
  if (subscribers.size > 0) request();
}

/** Abonne `fn` à la boucle ; renvoie la fonction qui le désabonne. */
export function onFrame(fn: Tick): () => void {
  subscribers.add(fn);
  if (!frame && !timer) request();
  return () => {
    subscribers.delete(fn);
    if (subscribers.size === 0) {
      cancelAnimationFrame(frame);
      window.clearTimeout(timer);
      frame = 0;
      timer = 0;
    }
  };
}

/** Nombre d'abonnés (tests). */
export function activeTicks(): number {
  return subscribers.size;
}
