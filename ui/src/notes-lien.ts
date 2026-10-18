// Depuis la mascotte, un clic sur la fiche de notes ou sur l'étiquette REC
// ouvre (ou ferme) le panneau des notes en direct (KOB-73). Au clavier,
// l'étiquette REC est un bouton (Entrée, Espace).

import './styles/notes-lien.css';
import { invoke } from './ipc';

const CIBLES = '.rec, .card-notes';

export function relierNotes(root: HTMLElement): void {
  const basculer = () => void invoke('basculer_notes').catch(() => {});
  root.addEventListener('click', (e) => {
    if ((e.target as Element | null)?.closest(CIBLES)) basculer();
  });
  const rec = root.querySelector<HTMLElement>('.rec');
  if (!rec) return;
  rec.setAttribute('role', 'button');
  rec.tabIndex = 0;
  rec.title = 'Voir les notes';
  rec.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      basculer();
    }
  });
}
