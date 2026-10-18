// Repos économe : toutes les boucles avancent par paliers (`steps()`) de
// 250 ms ou d'un multiple (voir kobi.css, bubbles.css, meeting.css). Recalées
// sur la même grille, leurs paliers tombent sur les mêmes images : au plus
// 4 images par seconde, au lieu d'une image par palier de chaque animation.
// Sans GPU, chaque image coûte (WebKitGTK repeint et recompose la fenêtre).

/** Palier commun du repos économe, en ms. */
export const STEP_MS = 250;

function stepped(a: Animation): boolean {
  const effect = a.effect as KeyframeEffect | null;
  return effect !== null && effect.getKeyframes().some((k) => String(k.easing).startsWith('steps'));
}

/**
 * Décale (de moins d'un palier, invisible) chaque animation à paliers en
 * cours sous `root` pour que ses paliers tombent sur la grille commune.
 */
export function alignSteps(root: Element): void {
  for (const a of root.getAnimations({ subtree: true })) {
    if (a.playState !== 'running' || a.startTime === null || !stepped(a)) continue;
    const start = Number(a.startTime) + Number(a.effect?.getTiming().delay ?? 0);
    const phase = ((start % STEP_MS) + STEP_MS) % STEP_MS;
    if (phase > 0.5 && phase < STEP_MS - 0.5) a.startTime = Number(a.startTime) - phase;
  }
}
