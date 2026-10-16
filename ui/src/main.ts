// Point d'entrée de l'interface. Paramètres d'URL (développement, tests,
// captures) : `skin`, `state`, `backdrop=1`, `reduced=1`.

import { isMascotState, STATES, type MascotData, type MascotState } from './contract';
import { demoData } from './demo';
import { skinFactory } from './skins/registry';
import { Stage } from './stage';
import './styles/fonts.css';
import './styles/stage.css';
import './styles/bubbles.css';

const params = new URLSearchParams(location.search);
const reduced = params.get('reduced');

const stage = new Stage(document.getElementById('app')!, {
  backdrop: params.get('backdrop') === '1',
  reducedMotion: reduced === null ? undefined : reduced === '1',
});
stage.useSkin(skinFactory(params.get('skin')));

const initial = params.get('state');
if (isMascotState(initial)) stage.setState(initial, demoData(initial));

/** Accès pour les tests, les captures et la coque native. */
declare global {
  interface Window {
    kobi: {
      stage: Stage;
      states: readonly MascotState[];
      setState(state: MascotState, data?: MascotData): void;
      update(data: MascotData): void;
      useSkin(id: string): void;
      demoData(state: MascotState): MascotData;
    };
  }
}

window.kobi = {
  stage,
  states: STATES,
  setState: (state, data) => stage.setState(state, data ?? {}),
  update: (data) => stage.update(data),
  useSkin: (id) => void stage.useSkin(skinFactory(id)),
  demoData,
};
