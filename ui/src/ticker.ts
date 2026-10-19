// Une seule boucle requestAnimationFrame pour toute l'interface : brouillage
// des caractères, frappe des bulles, volume du micro, suivi du curseur…
// Chaque abonné est appelé à chaque image avec l'heure de l'image et renvoie
// `true` pour être rappelé à l'image suivante. Sans abonné, la boucle s'arrête.

type Tick = (now: number) => boolean;

const subscribers = new Set<Tick>();
let frame = 0;

/** Intervalle imposé entre deux images (images par seconde réglées), 0 = chaque image. */
let interval = 0;
let timer = 0;
/** Heure de la dernière image servie aux abonnés. */
let last = Number.NEGATIVE_INFINITY;
/** Marge sous l'intervalle : une image de l'écran qui tombe un peu tôt compte. */
const TOLERANCE_MS = 4;
/** Attente minimale avant de laisser l'écran donner l'image (une image à 60 Hz, plus une marge). */
const AVANCE_MS = 20;

function request(): void {
  // Images par seconde réglées (KOB-200) : on attend en dormant jusqu'à peu
  // avant l'échéance, puis l'image de l'écran qui suit sert : la cadence reste
  // alignée sur l'écran, sans dérive (30 images/s à 60 Hz : une image sur deux).
  const reste = interval > 0 ? interval - (performance.now() - last) : 0;
  if (reste > AVANCE_MS) timer = window.setTimeout(() => (frame = requestAnimationFrame(loop)), reste - AVANCE_MS / 2);
  else frame = requestAnimationFrame(loop);
}

/**
 * Images par seconde réglées (econome.ts) : la boucle ne sert plus qu'une
 * image toutes les `ms` ; 0 la rend à chaque image de l'écran.
 */
export function setFrameInterval(ms: number): void {
  interval = ms;
}

function loop(now: number): void {
  frame = 0;
  timer = 0;
  if (interval > 0 && now - last < interval - TOLERANCE_MS) {
    request();
    return;
  }
  last = now;
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
