// Structure commune des réglages de Kobi. Chaque sujet (stockage, démarrage
// avec Windows, dossiers de code, mode de réponse…) est un `Panneau`
// autonome, monté dans la fenêtre Réglages (`reglages.html`) ou dans une
// autre fenêtre qui en a besoin (le Carnet monte « Stockage », le premier
// lancement pourra monter « Dossiers de code »).
//
// Ajouter un réglage : écrire un module qui exporte un `Panneau`, puis
// l'ajouter à `PANNEAUX` (ui/src/reglages/panneaux.ts).

export type Ton = 'ok' | 'erreur';

/** Où en est un réglage : prêt, à revoir, en panne, ou pas encore disponible. */
export type Statut = 'ok' | 'attention' | 'erreur' | 'attente';

/** Ce que la fenêtre hôte offre au panneau. */
export interface Contexte {
  /** Message court (enregistré, erreur…) affiché par l'hôte. */
  notifier(texte: string, ton?: Ton): void;
  /** État du réglage, montré par le premier lancement à côté de l'étape. */
  signaler?(statut: Statut, resume: string): void;
}

export interface Panneau {
  /** Identifiant stable, utilisé dans `?panneau=` et `ouvrir_reglages`. */
  id: string;
  titre: string;
  /** Une ligne sous le titre. */
  sousTitre: string;
  /** Remplit `el` ; retourne éventuellement de quoi nettoyer au démontage. */
  monter(el: HTMLElement, ctx: Contexte): void | (() => void);
}
