// Fausse coque Tauri de l'app Kobi pour les tests d'animation et les vidéos
// (KOB-151) : une réunion, un rappel, un journal, une réunion en cours.

export function fakeTauri(): void {
  const listeners: Record<string, Array<(e: { payload: unknown }) => void>> = {};
  const w = window as unknown as Record<string, unknown>;
  w.__emit = (event: string, payload: unknown) => (listeners[event] ?? []).forEach((h) => h({ payload }));
  const debut = new Date().setHours(9, 0, 0, 0);
  const reunions = [
    { nom: '2026-10-18_point-budget', date: '2026-10-18', titre: 'Point budget', debut_ms: debut, fin_ms: debut + 2_520_000, en_cours: false, duree_s: 2520, a_compte_rendu: false, apercu: 'Budget validé.' },
  ];
  const rappels = [{ id: 'r1', texte: 'Appeler Léa', echeance_ms: debut + 86_400_000, cree_ms: debut, fait: false, source: null }];
  w.__TAURI__ = {
    core: {
      invoke: async (cmd: string) => {
        switch (cmd) {
          case 'carnet_index':
            return { racine: '/home/test/Kobi', reunions, conversations: [], rappels };
          case 'carnet_chercher':
            return [];
          case 'carnet_rappel_fait':
            rappels[0]!.fait = true;
            return true;
          case 'reglages_stockage':
            return {
              dossier: '/home/test/Kobi',
              par_defaut: true,
              retention_reunions: 'unlimited',
              retention_conversations: 'unlimited',
              publication_reunions: { etat: 'aucune' },
              publication_conversations: { etat: 'aucune' },
            };
          case 'a_propos':
            return { version: '0.1.0', dossier: '/home/test/Kobi' };
          case 'etat_session':
            return { etat: 'veille', activite: null, mascotte: 'idle', transcrit: false };
          case 'etat_enregistrement':
            return { actif: true, pause: false, debut_ms: Date.now() - 58_000, notes: 2, titre: 'Point hebdo' };
          case 'journal_activite':
            return [{ quand_ms: Date.now(), genre: 'question', texte: 'Quelle heure est-il ?' }];
          case 'notes_en_cours':
            return null;
          default:
            throw `Command ${cmd} not found`;
        }
      },
    },
    event: {
      listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
        (listeners[event] ??= []).push(handler);
        return () => {};
      },
    },
  };
}
