// Clics qui traversent (KOB-217) : la fenêtre de Kobi est transparente et
// plus grande que lui. Partout où il n'y a ni Kobi ni un de ses affichages,
// la souris doit passer aux applications en dessous. L'interface envoie à la
// coque les rectangles de ses zones actives ; la coque suit le pointeur et
// laisse passer la souris hors de ces zones (traversee.rs).

import { tauri } from './native';

/** Kobi et ses affichages : la souris y reste à Kobi. */
export const ZONES_ACTIVES = [
  '.kobi .pill',
  '.kobi .word',
  '.card',
  '.rec',
  '.dot',
  '.kmenu',
  // Carte « quitter pendant une réunion », centrée hors du menu (KOB-264).
  '.kmenu-confirm',
  '.kmenu-toast',
  '.notifs > *',
  '.fil.shown',
  '.lecture',
  '.ecrire',
  '.rapport-badge',
].join(', ');

/** Marge autour de chaque zone (px) : on attrape un bord sans viser au pixel. */
const MARGE = 8;
/** Les rectangles sont arrondis à ce pas : Kobi qui respire ne renvoie rien. */
const PAS = 8;
/** Relevé des zones : assez souvent pour suivre une bulle qui s'ouvre. */
const RELEVE_MS = 200;

export type Zone = [x: number, y: number, largeur: number, hauteur: number];

/** Rectangles des zones actives visibles, en px de la fenêtre, marge comprise. */
export function zonesActives(racine: ParentNode = document): Zone[] {
  const zones: Zone[] = [];
  for (const el of Array.from(racine.querySelectorAll<HTMLElement>(ZONES_ACTIVES))) {
    if (el.hidden || el.closest('[hidden]')) continue;
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden' || Number(cs.opacity) === 0) continue;
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    const x = Math.floor((r.left - MARGE) / PAS) * PAS;
    const y = Math.floor((r.top - MARGE) / PAS) * PAS;
    const droite = Math.ceil((r.right + MARGE) / PAS) * PAS;
    const bas = Math.ceil((r.bottom + MARGE) / PAS) * PAS;
    zones.push([x, y, droite - x, bas - y]);
  }
  return zones;
}

/**
 * Envoie les zones à la coque quand elles changent, et l'état du bouton : la
 * coque ne laisse jamais passer la souris pendant un appui (une sélection
 * qui déborde de la bulle ne se casse pas).
 */
export function installerTraversee(): void {
  const api = tauri();
  if (!api) return;
  let envoye = '';
  const envoyer = (): void => {
    const zones = zonesActives();
    const texte = JSON.stringify(zones);
    if (texte === envoye) return;
    envoye = texte;
    api.core.invoke('zones_actives', { zones }).catch(() => {
      // Coque plus ancienne, sans la commande : la fenêtre garde la souris.
    });
  };
  envoyer();
  // Relevé seulement quand la fenêtre est à l'écran (sobriété, KOB-264).
  let releve = 0;
  const suivre = (): void => {
    window.clearInterval(releve);
    if (document.hidden) return;
    envoyer();
    releve = window.setInterval(envoyer, RELEVE_MS);
  };
  document.addEventListener('visibilitychange', suivre);
  suivre();
  let appui = false;
  const appuyer = (etat: boolean): void => {
    if (etat === appui) return;
    appui = etat;
    api.core.invoke('appui_souris', { appui: etat }).catch(() => {});
  };
  window.addEventListener('pointerdown', () => appuyer(true), true);
  window.addEventListener('pointerup', () => appuyer(false), true);
  window.addEventListener('pointercancel', () => appuyer(false), true);
  // Après un déplacement de la fenêtre, le relâchement n'arrive pas à la page.
  window.addEventListener('pointermove', (e) => appuyer(e.buttons !== 0), true);
}
