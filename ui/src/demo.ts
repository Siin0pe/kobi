// Données de démonstration (reprises du prototype) : chaque état avec des
// données complètes, pour le développement, les tests et les captures.

import type { MascotData, MascotState } from './states';

export const DEMO_QUESTION = 'où est géré le refresh token ? Il expire trop vite.';
export const DEMO_NOTES = 'Décision : refresh token → 30 j\nAction : mettre à jour la doc API\nQuestion : impact sur le mobile ?';
export const DEMO_ANSWER =
  'C’est dans api/src/auth/refresh.ts, ligne 88. Durée actuelle : 15 min. Un ticket ouvert en parle déjà.';

const ACTION = {
  id: 'demo-ticket',
  kicker: 'Je crée le ticket ?',
  title: 'Passer le refresh token à 30 jours',
  meta: 'linear · équipe api · d’après 23:38',
  doneKicker: 'Linear',
  doneTitle: 'Ticket créé.',
  doneMeta: 'Lien ajouté au compte rendu.',
} as const;

/** Données complètes de chaque état (texte entièrement tapé, sources trouvées). */
export function demoData(state: MascotState): MascotData {
  switch (state) {
    case 'idle':
      return {};
    case 'listen':
      return { question: DEMO_QUESTION };
    case 'think':
      return {
        sources: [
          { icon: '{}', label: 'codebase', found: true },
          { icon: '#', label: 'linear', found: true },
          { icon: '/', label: 'web', found: false },
        ],
      };
    case 'write':
      return { notes: DEMO_NOTES };
    case 'answer':
      return { answer: DEMO_ANSWER, answerTags: ['codebase', 'linear'] };
    case 'suggest':
      return { action: { ...ACTION, status: 'pending' } };
    case 'done':
      return { action: { ...ACTION, status: 'confirmed' } };
    case 'error':
      return {
        error: {
          title: 'Linear ne répond pas',
          message: 'Je réessaie dans 5 s. Le ticket est gardé dans le compte rendu.',
        },
      };
    case 'work':
      return { tool: 'recherche' };
  }
}

/**
 * Rejoue un état comme dans l'application : texte en flux, sources trouvées
 * une à une, compte à rebours de l'erreur. Rythme du prototype (pas de 60 ms).
 * Renvoie une fonction qui annule la lecture.
 */
export function playDemo(
  state: MascotState,
  apply: (state: MascotState, data: MascotData) => void,
  update: (data: MascotData) => void,
  level: (level: number) => void = () => {},
): () => void {
  const timers: number[] = [];
  const at = (ms: number, fn: () => void) => timers.push(window.setTimeout(fn, ms));
  const full = demoData(state);

  switch (state) {
    case 'think': {
      const all = full.sources!;
      apply(state, { sources: [] });
      all.forEach((s, i) => {
        at(360 + i * 840, () => update({ sources: all.slice(0, i + 1).map((x, j) => ({ ...x, found: j < i ? all[j]!.found : false })) }));
        if (s.found) at(960 + i * 840, () => update({ sources: all.slice(0, i + 1).map((x, j) => ({ ...x, found: j <= i ? all[j]!.found : false })) }));
      });
      break;
    }
    case 'answer': {
      // La réponse arrive par morceaux, comme un flux de Claude.
      const words = full.answer!.split(' ');
      apply(state, { answer: '', answerTags: full.answerTags });
      for (let i = 1; i <= words.length; i++) at(i * 140, () => update({ answer: words.slice(0, i).join(' ') }));
      break;
    }
    case 'listen': {
      // Volume d'une voix qui pose la question : syllabes et petites pauses.
      apply(state, full);
      for (let t = 0; t < 3200; t += 50) {
        const syllable = Math.max(0, Math.sin(t / 37) * Math.sin(t / 151));
        const pause = (t > 1300 && t < 1600) || t > 2900 ? 0 : 1;
        at(t, () => level(pause * (0.25 + 0.75 * syllable)));
      }
      break;
    }
    case 'error': {
      const message = (n: number) => `Je réessaie dans ${n} s. Le ticket est gardé dans le compte rendu.`;
      apply(state, { error: { ...full.error!, message: message(5) } });
      for (let n = 4; n >= 1; n--) at((5 - n) * 720, () => update({ error: { ...full.error!, message: message(n) } }));
      break;
    }
    case 'work': {
      // Kobi enchaîne les outils, comme sur une vraie question de code.
      const tools = ['recherche', 'lecture', 'commande', 'mcp', 'lecture'] as const;
      apply(state, {});
      tools.forEach((tool, i) => at(600 + i * 1400, () => update({ tool })));
      break;
    }
    default:
      apply(state, full);
  }
  return () => timers.forEach((t) => window.clearTimeout(t));
}
