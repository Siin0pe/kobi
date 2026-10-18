// Notes en direct (KOB-73) : le panneau ouvert d'un clic sur la fiche ou
// l'étiquette REC de Kobi. La dernière note en tête, puis Décisions, Actions
// et Questions, repliables. Lu par `notes_en_cours` (le `notes.md` de la
// réunion en cours) et rafraîchi sur les événements `notes` et
// `enregistrement`, avec une relecture régulière en secours.

import '../styles/fonts.css';
import '../reglages/reglages.css';
import './notes.css';
import { cleDuJour, heure, libelleDuJour, markdown } from '../carnet/format';
import { animationsReduites, h } from '../dom';
import { disponible, ecouter, invoke, message } from '../ipc';
import { Blinker } from '../mascotte/glyphs';
import { poserTitre } from '../titre';

/** Ce que renvoie `notes_en_cours`. */
export interface NotesEnCours {
  reunion: string;
  titre: string | null;
  debut_ms: number | null;
  decisions: string;
  actions: string;
  questions: string;
  autres: Array<[string, string]>;
  derniere_note: string | null;
  lignes: number;
  ecrit_ms: number | null;
}

/** Relecture de secours, tant que le panneau est visible. */
const RELECTURE_MS = 5_000;

const yeux = [h('span', { class: 'oeil' }, 'o'), h('span', { class: 'oeil' }, 'o')];
const titre = h('h1', { class: 'titre' });
const duree = h('span', { class: 'duree' });
const replier = h('button', { type: 'button', class: 'icone', 'data-action': 'replier', 'aria-label': 'Replier', 'aria-expanded': 'true' }, '–');
const fermer = h('button', { type: 'button', class: 'icone', 'data-action': 'fermer', 'aria-label': 'Fermer les notes' }, '×');
const derniere = h('section', { class: 'derniere', 'aria-live': 'polite', 'aria-label': 'Dernière note' });
const sections = h('div', { class: 'sections' });
const panneau = h(
  'div',
  { class: 'panneau-notes', 'data-replie': 'false' },
  h(
    'header',
    { class: 'tete', 'data-tauri-drag-region': true },
    h('span', { class: 'mini-kobi', 'aria-hidden': 'true', 'data-tauri-drag-region': true }, ...yeux),
    h('div', { class: 'tete-texte', 'data-tauri-drag-region': true }, h('span', { class: 'rec-pastille', 'data-tauri-drag-region': true }, h('span', { class: 'rec-point' }), 'REC', duree), titre),
    replier,
    fermer,
  ),
  derniere,
  sections,
);
document.getElementById('app')!.append(panneau);

// Kobi prend des notes : ses yeux regardent la page, et clignent.
const blinker = new Blinker((ferme) => yeux.forEach((o) => (o.textContent = ferme ? '–' : 'o')));
blinker.run(!animationsReduites());

let courant: NotesEnCours | null = null;
/** Puces déjà montrées : les nouvelles arrivent en glissant. */
const vues = new Set<string>();
/** Durée de la mise en valeur d'une puce nouvelle (`li.nouvelle`, notes.css). */
const NOUVELLE_MS = 1_600;
/** Puces nouvelles encore mises en valeur : un redessin reprend leur animation où elle en était. */
const nouveautes = new Map<string, number>();
let premierAffichage = true;

function compte(texte: string): number {
  return texte.split('\n').filter((l) => /^\s*(?:[-*+]|\d+[.)])\s+/.test(l)).length;
}

function section(cle: string, libelle: string, texte: string, vide: string): HTMLElement {
  const n = compte(texte);
  const details = h(
    'details',
    { class: 'section', 'data-section': cle, open: true },
    h('summary', {}, h('span', { class: 'section-titre' }, libelle), h('span', { class: 'nombre' }, String(n))),
    texte.trim() ? markdown(texte) : h('p', { class: 'vide' }, vide),
  );
  // Une section repliée le reste d'un rafraîchissement à l'autre.
  const ancienne = sections.querySelector<HTMLDetailsElement>(`details[data-section="${cle}"]`);
  if (ancienne && !ancienne.open) details.removeAttribute('open');
  // Le compte change : le nouveau chiffre arrive par le haut, comme le compteur REC de Kobi.
  const avant = ancienne?.querySelector('.nombre')?.textContent;
  if (avant !== undefined && avant !== String(n) && !animationsReduites()) {
    details.querySelector<HTMLElement>('.nombre')!.animate(
      [
        { transform: 'translateY(-90%) scale(1.3)', opacity: 0 },
        { transform: 'translateY(10%) scale(1.15)', opacity: 1, offset: 0.65 },
        { transform: 'none' },
      ],
      { duration: 420, easing: 'cubic-bezier(.2,.8,.3,1)' },
    );
  }
  return details;
}

function dessiner(n: NotesEnCours | null): void {
  courant = n;
  document.body.dataset.etat = n ? (n.lignes ? 'notes' : 'attente') : 'hors-reunion';
  if (!n) {
    titre.textContent = 'Pas de réunion';
    duree.textContent = '';
    derniere.replaceChildren(h('p', { class: 'vide' }, 'Les notes apparaissent ici pendant une réunion enregistrée.'));
    sections.replaceChildren();
    return;
  }
  const nom = n.titre ?? (n.debut_ms ? `Réunion ${libelleDuJour(cleDuJour(n.debut_ms)).toLowerCase()}` : 'Réunion');
  if (titre.getAttribute('aria-label') !== nom) poserTitre(titre, nom);
  tic();
  const blocs = [
    section('decisions', 'Décisions', n.decisions, 'Aucune décision pour l’instant.'),
    section('actions', 'Actions', n.actions, 'Aucune action pour l’instant.'),
    section('questions', 'Questions', n.questions, 'Aucune question ouverte.'),
    ...n.autres.map(([t, texte], i) => section(`autre-${i}`, t, texte, '')),
  ];
  sections.replaceChildren(...blocs);
  // Premier affichage : sections et puces entrent en cascade ; ensuite, seules
  // les nouveautés bougent (un rafraîchissement ne rejoue rien).
  sections.classList.toggle('premier', premierAffichage);
  if (premierAffichage) sections.querySelectorAll<HTMLElement>('.section, li').forEach((e, i) => e.style.setProperty('--i', String(i)));
  const nouvelles: HTMLElement[] = [];
  const maintenant = performance.now();
  for (const li of sections.querySelectorAll('li')) {
    const cle = li.textContent ?? '';
    if (!vues.has(cle)) {
      if (!premierAffichage) {
        nouveautes.set(cle, maintenant);
        nouvelles.push(li);
      }
      vues.add(cle);
    }
    const depuis = nouveautes.get(cle);
    if (depuis === undefined) continue;
    if (maintenant - depuis >= NOUVELLE_MS) nouveautes.delete(cle);
    else {
      li.classList.add('nouvelle');
      li.style.animationDelay = `-${Math.round(maintenant - depuis)}ms`;
    }
  }
  // La dernière note est la dernière puce apparue (une décision ajoutée en
  // haut compte autant qu'une question en bas), sinon la dernière du fichier.
  const texte = nouvelles.at(-1)?.textContent?.trim() || n.derniere_note;
  if (texte) {
    if (derniere.dataset.texte !== texte) {
      derniere.dataset.texte = texte;
      const ligne = h('p', { class: 'derniere-texte' });
      derniere.replaceChildren(
        h('p', { class: 'derniere-etiquette' }, 'Dernière note', n.ecrit_ms ? h('span', { class: 'quand' }, ` · ${heure(n.ecrit_ms)}`) : null),
        ligne,
      );
      // La note s'écrit avec le brouillage de Kobi.
      poserTitre(ligne, texte);
      if (!premierAffichage) {
        // Relance l'animation d'arrivée.
        derniere.classList.remove('arrive');
        void derniere.offsetWidth;
        derniere.classList.add('arrive');
      }
    }
  } else {
    delete derniere.dataset.texte;
    derniere.replaceChildren(h('p', { class: 'vide' }, 'J’écoute… les premières notes arrivent dans une minute.'));
  }
  premierAffichage = false;
}

/** Durée de la réunion, mise à jour chaque seconde. */
function tic(): void {
  if (!courant?.debut_ms) return;
  const s = Math.max(0, Math.floor((Date.now() - courant.debut_ms) / 1000));
  const hh = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(hh ? 2 : 1, '0');
  const ss = String(s % 60).padStart(2, '0');
  duree.textContent = ` · ${hh ? `${hh}:` : ''}${mm}:${ss}`;
}
window.setInterval(tic, 1000);

let enCours = false;
async function rafraichir(): Promise<void> {
  if (enCours) return;
  enCours = true;
  try {
    dessiner(await invoke<NotesEnCours | null>('notes_en_cours'));
  } catch (e) {
    derniere.replaceChildren(h('p', { class: 'erreur' }, message(e)));
  } finally {
    enCours = false;
  }
}

// L'écrivain range `notes.md` sur un fil à part : on relit aussitôt, puis un
// peu après, pour ne pas lire le fichier avant son écriture.
let minuteries: number[] = [];
function bientot(): void {
  minuteries.forEach((t) => window.clearTimeout(t));
  minuteries = [150, 1500].map((ms) => window.setTimeout(() => void rafraichir(), ms));
}
ecouter('notes', bientot);
ecouter('enregistrement', bientot);
window.setInterval(() => {
  if (!document.hidden) void rafraichir();
}, RELECTURE_MS);
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) void rafraichir();
});

replier.addEventListener('click', () => {
  const replie = panneau.dataset.replie !== 'true';
  panneau.dataset.replie = String(replie);
  replier.setAttribute('aria-expanded', String(!replie));
  replier.setAttribute('aria-label', replie ? 'Déplier' : 'Replier');
  replier.textContent = replie ? '+' : '–';
  void invoke('replier_notes', { replie }).catch(() => {});
});
fermer.addEventListener('click', () => void invoke('fermer_notes').catch(() => {}));
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') void invoke('fermer_notes').catch(() => {});
});

/** Accès pour les tests. */
declare global {
  interface Window {
    notes: { rafraichir(): Promise<void> };
  }
}
window.notes = { rafraichir };

if (disponible()) void rafraichir();
else dessiner(null);
