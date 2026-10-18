// Point d'entrée de l'interface. Paramètres d'URL (développement, tests,
// captures) : `state`, `backdrop=1|sombre|clair`, `reduced=1`, `discret=1`,
// `reunion=<titre>` (+ `depuis=<s>`, `carte=0`).

import { isMascotState, STATES, type MascotData, type MascotState } from './states';
import { demoData, playDemo } from './demo';
import { command, enableWindowDrag, followSession, type Command, type SessionState } from './native';
import { applyRecording, type RecordingState } from './recording';
import { relierNotes } from './notes-lien';
import { Stage } from './stage';
import { applyCarte, enableInterrupt, followCartes, type CarteUi } from './cartes';
import { Conversation, followConversation, type Bulle } from './conversation';
import { Tasks } from './taches';
import './styles/fonts.css';
import './styles/stage.css';
import './styles/bubbles.css';
import './styles/meeting.css';
import './styles/conversation.css';

const params = new URLSearchParams(location.search);

function backdropParam(value: string | null): boolean | 'sombre' | 'clair' {
  return value === 'sombre' || value === 'clair' ? value : value === '1';
}
const reduced = params.get('reduced');

const stage = new Stage(document.getElementById('app')!, {
  backdrop: backdropParam(params.get('backdrop')),
  reducedMotion: reduced === null ? undefined : reduced === '1',
});
enableWindowDrag(stage.root);
relierNotes(stage.root);
followSession(stage);

// Conversation (KOB-133), tâches longues (KOB-135), cartes et interruption.
const conversation = new Conversation(stage);
const tasks = new Tasks(stage);
followConversation(conversation, (bulles) => tasks.update(bulles));
enableInterrupt(stage, conversation, followCartes(stage));

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
      /** Volume du micro (0 à 1), comme l'événement `niveau` de la coque. */
      setLevel(level: number): void;
      /** Réunion : indicateur REC et carte « prévenez les participants ». */
      startMeeting(title?: string): void;
      endMeeting(): void;
      /** État d'enregistrement, comme l'événement `enregistrement` de la coque. */
      recording(state: RecordingState): void;
      /** Mode discret (pastille de 32 px). */
      setCompact(compact: boolean): void;
      demoData(state: MascotState): MascotData;
      /** Rejoue un état avec ses données en flux (démo, captures). */
      play(state: MascotState): void;
      /** Commande envoyée à la session (coque native). */
      command(name: Command): Promise<SessionState | undefined>;
      /** Historique, comme l'événement `conversation` de la coque. */
      conversation(bulles: Bulle[]): void;
      /** Carte de confirmation, comme l'événement `carte` de la coque. */
      carte(carte: CarteUi): void;
      /** Échap : la réponse en cours s'arrête net. */
      interrupt(): void;
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
  setLevel: (level) => stage.setLevel(level),
  setCompact: (compact) => stage.setCompact(compact),
  startMeeting: (title) => stage.startMeeting({ title, startedAt: Date.now() }),
  endMeeting: () => stage.endMeeting(),
  recording: (state) => applyRecording(stage, state),
  demoData,
  command,
  conversation: (bulles) => {
    conversation.setBulles(bulles);
    tasks.update(bulles);
  },
  carte: (c) => applyCarte(stage, c),
  interrupt: () => stage.interrupt(),
  play: (state) => {
    stopDemo();
    stopDemo = playDemo(
      state,
      (s, d) => stage.setState(s, d),
      (d) => stage.update(d),
      (level) => stage.setLevel(level),
    );
  },
};
