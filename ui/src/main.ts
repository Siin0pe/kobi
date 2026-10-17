// Point d'entrée de l'interface. Paramètres d'URL (développement, tests,
// captures) : `skin`, `state`, `backdrop=1`, `reduced=1`.

import { isMascotState, STATES, type MascotData, type MascotState } from './contract';
import { demoData, playDemo } from './demo';
import { enableWindowDrag } from './native';
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
enableWindowDrag(stage.root);

let stopDemo: () => void = () => {};

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
      /** Rejoue un état avec ses données en flux (démo, captures). */
      play(state: MascotState): void;
    };
  }
}

window.kobi = {
  stage,
  states: STATES,
  setState: (state, data) => {
    stopDemo();
    stage.setState(state, data ?? {});
  },
  update: (data) => stage.update(data),
  useSkin: (id) => void stage.useSkin(skinFactory(id)),
  demoData,
  play: (state) => {
    stopDemo();
    stopDemo = playDemo(
      state,
      (s, d) => stage.setState(s, d),
      (d) => stage.update(d),
    );
  },
};
