// Point d'entrée de l'interface. Paramètres d'URL (développement, tests,
// captures) : `skin`, `state`, `backdrop=1`, `reduced=1`, `discret=1`,
// `reunion=<titre>` (+ `depuis=<s>`, `carte=0`).

import { isMascotState, STATES, type MascotData, type MascotState } from './contract';
import { demoData, playDemo } from './demo';
import { command, enableWindowDrag, followSession, type Command, type SessionState } from './native';
import { loadSkin, onSkinChange } from './skin-choice';
import { skinFactory } from './skins/registry';
import { Stage } from './stage';
import './styles/fonts.css';
import './styles/stage.css';
import './styles/bubbles.css';
import './styles/meeting.css';

const params = new URLSearchParams(location.search);
const reduced = params.get('reduced');

const stage = new Stage(document.getElementById('app')!, {
  backdrop: params.get('backdrop') === '1',
  reducedMotion: reduced === null ? undefined : reduced === '1',
});
// Skin imposé par l'URL (tests, captures), sinon celui choisi dans les réglages.
const forced = params.get('skin');
stage.useSkin(skinFactory(forced ?? (await loadSkin())));
if (!forced) onSkinChange((id) => stage.useSkin(skinFactory(id)));
enableWindowDrag(stage.root);
followSession(stage);

let stopDemo: () => void = () => {};

if (params.get('discret') === '1') stage.setCompact(true);
const meeting = params.get('reunion');
if (meeting !== null) stage.startMeeting({ title: meeting || undefined, startedAt: Date.now() - Number(params.get('depuis') ?? 0) * 1000 }, { consent: params.get('carte') !== '0' });

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
      /** Réunion : indicateur REC et carte « prévenez les participants ». */
      startMeeting(title?: string): void;
      endMeeting(): void;
      /** Mode discret (pastille de 32 px). */
      setCompact(compact: boolean): void;
      demoData(state: MascotState): MascotData;
      /** Rejoue un état avec ses données en flux (démo, captures). */
      play(state: MascotState): void;
      /** Commande envoyée à la session (coque native). */
      command(name: Command): Promise<SessionState | undefined>;
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
  setCompact: (compact) => stage.setCompact(compact),
  startMeeting: (title) => stage.startMeeting({ title, startedAt: Date.now() }),
  endMeeting: () => stage.endMeeting(),
  demoData,
  command,
  play: (state) => {
    stopDemo();
    stopDemo = playDemo(
      state,
      (s, d) => stage.setState(s, d),
      (d) => stage.update(d),
    );
  },
};
