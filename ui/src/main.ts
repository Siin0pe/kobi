// Point d'entrée de l'interface. Paramètres d'URL (développement, tests,
// captures) : `state`, `backdrop=1|sombre|clair`, `reduced=1`, `discret=1`,
// `reunion=<titre>` (+ `depuis=<s>`, `carte=0`).

import './erreurs';
import { isMascotState, STATES, type MascotData, type MascotState } from './states';
import { demoData, playDemo } from './demo';
import { command, enableWindowDrag, followSession, tauri, type Command, type SessionState } from './native';
import { KobiMenu } from './menu';
import { applyRecording, type RecordingState } from './recording';
import { relierNotes } from './notes-lien';
import { Stage } from './stage';
import { applyCarte, enableInterrupt, followCartes, type CarteUi } from './cartes';
import { Conversation, followConversation, type Bulle } from './conversation';
import { Tasks, followRappels, type Rappel } from './taches';
import { Ecrire } from './ecrire';
import { followReport, ReportCard, type CompteRenduUi } from './compte-rendu';
import { Activite, followActivite, type Etape } from './activite';
import { Vue } from './vue';
import './styles/fonts.css';
import './styles/stage.css';
import './styles/bubbles.css';
import './styles/meeting.css';
import './styles/conversation.css';
import './styles/ecrire.css';
import './styles/compte-rendu.css';
import './styles/activite.css';
import './styles/menu.css';
import './styles/reponse.css';
import './styles/vue.css';

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
const menu = new KobiMenu(stage);

// Conversation (KOB-133), tâches longues (KOB-135), cartes et interruption.
const conversation = new Conversation(stage);
const tasks = new Tasks(stage);
// Fil d'activité sous Kobi (KOB-152).
const activite = new Activite(stage);
followActivite(activite);
// Vue agrandie de la conversation (KOB-169).
const vue = new Vue(stage);
followConversation(conversation, (bulles) => {
  tasks.update(bulles);
  vue.setBulles(bulles);
});
followRappels(tasks);
// Écrire à Kobi (KOB-147) : la bulle qui sort de Kobi.
const ecrire = new Ecrire(stage);
ecrire.onSend = (texte) => conversation.send(texte);
ecrire.onHistory = () => conversation.openHistory();
// Un clic sur la bulle de réponse (ou « Lire en grand ») ouvre la vue agrandie.
stage.onReadAll(() => void vue.ouvrir());
ecrire.yields = () => conversation.historyOpen || ['answer', 'think', 'listen'].includes(stage.currentState);
enableInterrupt(stage, conversation, followCartes(stage));
// Compte rendu de fin de réunion (KOB-74).
const report = new ReportCard(stage);
followReport(report);

let stopDemo: () => void = () => {};

if (params.get('discret') === '1') stage.setCompact(true);
// Mode discret demandé par la CLI ou le MCP de Kobi (KOB-161).
void tauri()?.event.listen<boolean>('kobi://discret', (e) => stage.setCompact(e.payload));
const meeting = params.get('reunion');
if (meeting !== null) stage.startMeeting({ title: meeting || undefined, startedAt: Date.now() - Number(params.get('depuis') ?? 0) * 1000 }, { consent: params.get('carte') !== '0' });

const initial = params.get('state');
if (isMascotState(initial)) stage.setState(initial, demoData(initial));

/** Accès pour les tests, les captures et la coque native. */
declare global {
  interface Window {
    kobi: {
      stage: Stage;
      /** Menu du clic droit. */
      menu: KobiMenu;
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
      /** Rappel à l'heure dite, comme l'événement `rappel` de la coque. */
      rappel(r: Rappel): void;
      /** Carte de confirmation, comme l'événement `carte` de la coque. */
      carte(carte: CarteUi): void;
      /** Échap : la réponse en cours s'arrête net. */
      interrupt(): void;
      /** Ouvre la bulle pour écrire à Kobi (menu, raccourci). */
      ecrire(): void;
      /** Compte rendu de fin de réunion, comme l'événement `compte_rendu` de la coque. */
      compteRendu(c: CompteRenduUi): void;
      /** Étape du fil d'activité, comme l'événement `activite` de la coque. */
      activite(etape: Etape): void;
    };
  }
}

window.kobi = {
  stage,
  menu,
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
    vue.setBulles(bulles);
  },
  rappel: (r) => tasks.reminder(r),
  carte: (c) => applyCarte(stage, c),
  interrupt: () => stage.interrupt(),
  ecrire: () => ecrire.open(),
  compteRendu: (c) => report.show(c),
  activite: (etape) => activite.push(etape),
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
