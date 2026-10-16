// Skin factice : vérifie le contrat sans dessin. Un disque coloré selon
// l'état, le libellé de l'état et le nombre d'appels reçus.

import { STATE_LABELS, type MascotData, type MascotState, type Skin, type SkinEnv } from '../../contract';
import './factice.css';

const COLORS: Record<MascotState, string> = {
  idle: '#8A8F98',
  listen: '#2F80ED',
  think: '#9B51E0',
  write: '#27AE60',
  answer: '#00A3A3',
  suggest: '#F2994A',
  done: '#219653',
  error: '#EB5757',
};

export function createFakeSkin(): Skin {
  let root: HTMLElement | null = null;
  let dot: HTMLElement | null = null;
  let label: HTMLElement | null = null;
  let calls = 0;

  const render = (state: MascotState, data: Readonly<MascotData>): void => {
    if (!root || !dot || !label) return;
    calls += 1;
    root.dataset.state = state;
    root.dataset.calls = String(calls);
    root.dataset.hasData = String(Object.keys(data).length > 0);
    dot.style.background = COLORS[state];
    label.textContent = STATE_LABELS[state];
  };
  let state: MascotState = 'idle';

  return {
    id: 'factice',
    name: 'Factice',
    theme: {
      backdrop: '#EDEFF2',
      ink: '#1B1F24',
      paper: '#FFFFFF',
      accent: '#2F80ED',
      muted: '#5B6470',
      danger: '#EB5757',
      font: "'Recursive', system-ui, sans-serif",
    },
    mount(host: HTMLElement, env: SkinEnv) {
      root = document.createElement('div');
      root.className = 'skin-factice';
      root.classList.toggle('reduced-motion', env.reducedMotion);
      dot = document.createElement('div');
      dot.className = 'dot';
      label = document.createElement('div');
      label.className = 'label';
      root.append(dot, label);
      host.append(root);
    },
    setState(next, data) {
      state = next;
      render(state, data);
    },
    setData(data) {
      render(state, data);
    },
    setPaused(paused) {
      root?.classList.toggle('paused', paused);
    },
    setReducedMotion(reduced) {
      root?.classList.toggle('reduced-motion', reduced);
    },
    destroy() {
      root?.remove();
      root = dot = label = null;
    },
  };
}
