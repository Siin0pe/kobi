// Fenêtre du menu du clic droit (KOB-202) : une petite fenêtre transparente,
// sans bordure et toujours au-dessus, que la coque pose sous le pointeur,
// n'importe où sur l'écran ; la mascotte ne bouge pas. Comme un menu
// contextuel Windows, le menu a son coin haut-gauche sous le pointeur et se
// retourne vers la gauche ou le haut s'il déborde de la zone de travail de
// l'écran (`placer`). Il se ferme comme un menu natif : clic ailleurs (la
// fenêtre perd le focus), Échap, choix d'une entrée.
//
// Même carte, mêmes entrées, même clavier et même éventail que dans la
// fenêtre de Kobi (menu.ts) ; l'entrée choisie repart à la mascotte
// (`menu://choix`), qui l'exécute comme un clic.

import { suivreClasseAnimations } from './animations';
import { initLangue, surLangue, t } from './i18n';
import { clavierMenu, remplirMenu, type MenuContext, type MenuId } from './menu';
import { placer, type Point, type Zone } from './menu-place';
import './styles/fonts.css';
import './styles/stage.css';
import './styles/menu.css';
import './styles/menu-fenetre.css';

/** Ce que la coque envoie à l'ouverture (`menu_souris.rs`). */
export interface Ouverture {
  /** Pointeur, en pixels physiques de l'écran. */
  pointeur: Point;
  /** Zone de travail de l'écran du pointeur, en pixels physiques. */
  zone: Zone;
  /** Échelle d'affichage de cet écran. */
  echelle: number;
  contexte: MenuContext;
}

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> };
}

/** Autour de la carte, dans la fenêtre : son liseré (tout autour) et son ombre dure (à droite, en bas). */
const BORD = 3;
const OMBRE = 6;

function tauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

suivreClasseAnimations();
// La langue de l'interface (KOB-209) : le menu suit les changements à chaud.
void initLangue();
const root = document.createElement('div');
root.className = 'kmenu';
root.hidden = true;
root.setAttribute('role', 'menu');
root.setAttribute('aria-label', t('kobi.menu.aria'));
surLangue(() => root.setAttribute('aria-label', t('kobi.menu.aria')));
root.setAttribute('aria-orientation', 'vertical');
const heading = document.createElement('div');
heading.className = 'kmenu-head';
heading.setAttribute('aria-hidden', 'true');
const list = document.createElement('div');
list.className = 'kmenu-list';
root.append(heading, list);
document.body.append(root);

let items: HTMLButtonElement[] = [];
let ouvert = false;

/** Le menu se range ; `entree` : l'entrée choisie, à exécuter par la mascotte. */
function fermer(entree: MenuId | null = null): void {
  if (!ouvert) return;
  ouvert = false;
  const rendre = (): void => {
    root.hidden = true;
    root.classList.remove('opening');
    void tauri()?.core.invoke('menu_choisi', { entree });
  };
  // L'entrée choisie s'allume un instant, comme dans la fenêtre de Kobi.
  if (entree && !document.documentElement.classList.contains('reduced-motion')) window.setTimeout(rendre, 120);
  else rendre();
}

function choisir(id: MenuId): void {
  items.find((b) => b.dataset.id === id)?.classList.add('chosen');
  fermer(id);
}

/** Ouvre le menu à la place calculée, puis la demande à la coque. */
export async function ouvrir(o: Ouverture): Promise<void> {
  items = remplirMenu(heading, list, o.contexte, choisir);
  root.hidden = false;
  root.classList.remove('opening');
  // Mesuré avec sa police : à la première ouverture, Recursive se charge encore.
  await document.fonts.ready;
  // Taille de mise en page : l'animation d'ouverture (scale) n'y compte pas.
  const width = root.offsetWidth;
  const height = root.offsetHeight;
  const e = o.echelle > 0 ? o.echelle : 1;
  const place = placer(o.pointeur, width * e, height * e, o.zone);
  // L'éventail part du coin qui est sous le pointeur.
  root.style.setProperty('--ox', place.versGauche ? `${width}px` : '0px');
  root.style.setProperty('--oy', place.versHaut ? `${height}px` : '0px');
  root.dataset.coin = `${place.versHaut ? 'bas' : 'haut'}-${place.versGauche ? 'droite' : 'gauche'}`;
  ouvert = true;
  try {
    await tauri()?.core.invoke('montrer_menu', {
      x: Math.round(place.x - BORD * e),
      y: Math.round(place.y - BORD * e),
      largeur: Math.ceil((width + BORD + OMBRE) * e),
      hauteur: Math.ceil((height + BORD + OMBRE) * e),
    });
  } catch {
    // Fenêtre jamais montrée (refus du gestionnaire de fenêtres…) : aucun
    // `blur` ne la fermerait ; la mascotte retrouve tout de suite son état.
    fermer();
    return;
  }
  void root.offsetWidth;
  root.classList.add('opening');
  items[0]?.focus({ preventScroll: true });
}

root.addEventListener('keydown', (e) => clavierMenu(e, items, choisir, () => fermer()));
// Clic ailleurs, y compris hors de Kobi : la fenêtre du menu perd le focus.
window.addEventListener('blur', () => fermer());
document.addEventListener('contextmenu', (e) => e.preventDefault());
document.addEventListener('selectstart', (e) => e.preventDefault());

const api = tauri();
if (api) void api.event.listen<Ouverture>('menu://ouvrir', (e) => void ouvrir(e.payload));
// Navigateur et tests : la page s'ouvre par `window.__ouvrirMenu`.
(window as unknown as { __ouvrirMenu?: (o: Ouverture) => Promise<void> }).__ouvrirMenu = ouvrir;
