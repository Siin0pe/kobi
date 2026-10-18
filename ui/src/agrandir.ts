// Réponse longue (KOB-163) : la fenêtre de Kobi s'agrandit vers le côté libre
// de l'écran le temps de la lire, au lieu de tasser la bulle dans 410 × 280,
// puis reprend sa taille. La coque calcule le sens (`agrandir_kobi`), la
// scène se range dans son coin, puis la fenêtre grandit : Kobi ne bouge pas.

import { tauri } from './native';
import type { Agrandie, Stage } from './stage';

/** Longueur de réponse (caractères) à partir de laquelle la fenêtre s'agrandit. */
export const GROW_AT = 160;
/** Délai avant de reprendre la taille une fois la réponse partie (la souris dessus le prolonge). */
const SHRINK_MS = 1_200;

export function followLongAnswers(stage: Stage): void {
  const api = tauri();
  if (!api) return;
  const invoke = (grand: boolean, appliquer: boolean) => api.core.invoke<Agrandie | null>('agrandir_kobi', { grand, appliquer });
  let busy = false;
  let timer = 0;
  let hovered = false;
  stage.root.addEventListener('pointerover', (e) => (hovered = (e.target as Element | null)?.closest('.card-answer') != null));
  stage.root.addEventListener('pointerleave', () => (hovered = false));

  const nextResize = (): Promise<void> =>
    new Promise((resolve) => {
      const done = (): void => {
        window.removeEventListener('resize', done);
        window.clearTimeout(t);
        resolve();
      };
      const t = window.setTimeout(done, 400);
      window.addEventListener('resize', done);
    });

  const grow = async (): Promise<void> => {
    busy = true;
    try {
      const sens = await invoke(true, false);
      if (!sens || stage.currentState !== 'answer') return;
      stage.freezeScale(true);
      stage.setAgrandie(sens);
      const resized = nextResize();
      await invoke(true, true);
      await resized;
      stage.freezeScale(false);
    } catch {
      stage.freezeScale(false);
      stage.setAgrandie(null);
    } finally {
      busy = false;
    }
  };

  const shrink = async (): Promise<void> => {
    if (hovered) {
      timer = window.setTimeout(() => void shrink(), SHRINK_MS);
      return;
    }
    busy = true;
    try {
      // La scène reste dans son coin pendant que la fenêtre rapetisse, puis se recentre.
      stage.freezeScale(true);
      const resized = nextResize();
      await invoke(false, true);
      await resized;
    } catch {
      // Fenêtre déjà à sa taille.
    } finally {
      stage.setAgrandie(null);
      stage.freezeScale(false);
      busy = false;
    }
  };

  const previous = stage.onRender;
  stage.onRender = (state, data) => {
    previous?.(state, data);
    if (state === 'answer') {
      window.clearTimeout(timer);
      timer = 0;
      if (!stage.agrandie && !busy && (data.answer ?? '').length >= GROW_AT) void grow();
    } else if (stage.agrandie && !timer) {
      timer = window.setTimeout(() => {
        timer = 0;
        if (stage.currentState !== 'answer') void shrink();
      }, SHRINK_MS);
    }
  };
}
