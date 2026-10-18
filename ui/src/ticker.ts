// Une seule boucle requestAnimationFrame pour toute l'interface : brouillage
// des caractères, frappe des bulles, volume du micro, suivi du curseur…
// Chaque abonné est appelé à chaque image avec l'heure de l'image et renvoie
// `true` pour être rappelé à l'image suivante. Sans abonné, la boucle s'arrête.

type Tick = (now: number) => boolean;

const subscribers = new Set<Tick>();
let frame = 0;

function loop(now: number): void {
  frame = 0;
  for (const fn of [...subscribers]) {
    if (!subscribers.has(fn)) continue;
    let again = false;
    try {
      again = fn(now);
    } finally {
      if (!again) subscribers.delete(fn);
    }
  }
  if (subscribers.size > 0) frame = requestAnimationFrame(loop);
}

/** Abonne `fn` à la boucle ; renvoie la fonction qui le désabonne. */
export function onFrame(fn: Tick): () => void {
  subscribers.add(fn);
  if (!frame) frame = requestAnimationFrame(loop);
  return () => {
    subscribers.delete(fn);
    if (subscribers.size === 0 && frame) {
      cancelAnimationFrame(frame);
      frame = 0;
    }
  };
}

/** Nombre d'abonnés (tests). */
export function activeTicks(): number {
  return subscribers.size;
}
