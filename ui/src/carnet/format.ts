// Dates, durées et petit rendu markdown du Carnet (heure locale, français).

import { h, surligner } from '../dom';

const JOUR = 86_400_000;

/** Clé de jour local `AAAA-MM-JJ`. */
export function cleDuJour(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

/** Date d'un dossier `AAAA-MM-JJ` à midi local (évite les bascules d'heure d'été). */
function depuisCle(cle: string): Date {
  const [a, m, j] = cle.split('-').map(Number);
  return new Date(a!, (m ?? 1) - 1, j ?? 1, 12);
}

/** « Aujourd'hui », « Hier », sinon « samedi 17 octobre » (+ année si autre). */
export function libelleDuJour(cle: string, maintenant = Date.now()): string {
  const d = depuisCle(cle);
  const auj = depuisCle(cleDuJour(maintenant));
  const ecart = Math.round((auj.getTime() - d.getTime()) / JOUR);
  if (ecart === 0) return 'Aujourd’hui';
  if (ecart === 1) return 'Hier';
  return d.toLocaleDateString('fr-FR', {
    weekday: 'long',
    day: 'numeric',
    month: 'long',
    year: d.getFullYear() === auj.getFullYear() ? undefined : 'numeric',
  });
}

export function heure(ms: number): string {
  return new Date(ms).toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' });
}

/** « 42 min », « 1 h 05 », « 40 s ». */
export function duree(secondes: number): string {
  const s = Math.max(0, Math.round(secondes));
  if (s < 60) return `${s} s`;
  const min = Math.round(s / 60);
  if (min < 60) return `${min} min`;
  return `${Math.floor(min / 60)} h ${String(min % 60).padStart(2, '0')}`;
}

/** Horodatage d'un segment : `MM:SS`, ou `H:MM:SS` au-delà d'une heure. */
export function horodatage(secondes: number): string {
  const s = Math.max(0, Math.floor(secondes));
  const hh = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return hh > 0 ? `${hh}:${mm}:${ss}` : `${mm}:${ss}`;
}

/** Échéance d'un rappel : « aujourd'hui 14:30 », « demain 9:00 », « en retard ». */
export function echeance(ms: number | null, maintenant = Date.now()): { texte: string; retard: boolean } {
  if (ms === null) return { texte: 'sans date', retard: false };
  const jour = libelleDuJour(cleDuJour(ms), maintenant);
  const demain = cleDuJour(maintenant + JOUR) === cleDuJour(ms);
  return { texte: `${demain ? 'Demain' : jour} · ${heure(ms)}`, retard: ms < maintenant };
}

/** Texte en ligne : **gras** et `code`, le reste en texte brut (surligné). */
function enLigne(texte: string, mot: string): Node[] {
  const out: Node[] = [];
  const motif = /\*\*([^*]+)\*\*|`([^`]+)`/g;
  let i = 0;
  for (const m of texte.matchAll(motif)) {
    out.push(...surligner(texte.slice(i, m.index), mot));
    if (m[1] !== undefined) out.push(h('strong', {}, ...surligner(m[1], mot)));
    else out.push(h('code', {}, ...surligner(m[2]!, mot)));
    i = m.index! + m[0].length;
  }
  out.push(...surligner(texte.slice(i), mot));
  return out;
}

/**
 * Markdown simple d'un compte rendu ou de notes : listes (`-`, `*`, `1.`,
 * cases `[ ]`/`[x]`), titres `###`, paragraphes. Jamais de HTML brut.
 */
export function markdown(texte: string, mot = ''): HTMLElement {
  const racine = h('div', { class: 'md' });
  let liste: HTMLElement | null = null;
  let para: string[] = [];
  const finirPara = () => {
    if (para.length) racine.append(h('p', {}, ...enLigne(para.join(' '), mot)));
    para = [];
  };
  for (const brute of texte.split('\n')) {
    const ligne = brute.trim();
    const puce = /^(?:[-*+]|\d+[.)])\s+(.*)$/.exec(ligne);
    if (puce) {
      finirPara();
      const ordonnee = /^\d/.test(ligne);
      if (!liste || (liste.tagName === 'OL') !== ordonnee) {
        liste = h(ordonnee ? 'ol' : 'ul');
        racine.append(liste);
      }
      const caseACocher = /^\[( |x|X)\]\s*(.*)$/.exec(puce[1]!);
      liste.append(
        caseACocher
          ? h('li', { class: 'tache', 'data-fait': String(caseACocher[1] !== ' ') }, h('span', { class: 'case', 'aria-hidden': 'true' }), ...enLigne(caseACocher[2]!, mot))
          : h('li', {}, ...enLigne(puce[1]!, mot)),
      );
      continue;
    }
    liste = null;
    if (!ligne) {
      finirPara();
      continue;
    }
    const titre = /^#{1,6}\s+(.*)$/.exec(ligne);
    if (titre) {
      finirPara();
      racine.append(h('h4', {}, ...enLigne(titre[1]!, mot)));
      continue;
    }
    para.push(ligne);
  }
  finirPara();
  return racine;
}
