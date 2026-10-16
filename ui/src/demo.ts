// Données de démonstration (reprises du prototype) : chaque état avec des
// données complètes, pour le développement, les tests et les captures.

import type { MascotData, MascotState } from './contract';

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
  }
}
