// Menu du clic droit sur Kobi (KOB-139), aux couleurs de Kobi, à la place du
// menu et de la sélection du navigateur. Mêmes entrées que la barre système
// (`crates/kobi-app/src/menu.rs`), libellés suivant l'état (réunion, pause,
// mode discret).
//
// Dans l'application, chaque entrée part à la coque (`menu_kobi`), qui la
// journalise, agit (réunion par le chef, fenêtres Carnet et Réglages, micro)
// et renvoie l'événement `kobi://menu` ; l'interface fait le reste (mode
// discret, confirmation avant de quitter). Hors de Tauri (navigateur, tests), l'interface simule la
// coque pour que le menu reste essayable.
//
// Clavier : touche Menu ou Maj+F10 pour l'ouvrir, flèches, Origine/Fin,
// première lettre, Entrée ou Espace, Échap pour fermer.

import type { Stage } from './stage';

export const MENU_IDS = ['parler', 'ecrire', 'enregistrer', 'pause', 'app', 'carnet', 'reglages', 'voix', 'discret', 'quitter'] as const;
export type MenuId = (typeof MENU_IDS)[number];

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> };
}

function tauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

const SVG = 'http://www.w3.org/2000/svg';

/** Pictogrammes au trait, comme les glyphes de Kobi (viewBox 16×16). */
const ICONS: Record<string, string> = {
  parler: 'M2.5 3.5h11v7h-6l-3.5 3v-3h-1.5z',
  ecrire: 'M3 13l1-3.5l7-7l2.5 2.5l-7 7zM9.5 4l2.5 2.5',
  enregistrer: 'M8 3.2a4.8 4.8 0 1 0 0 9.6a4.8 4.8 0 1 0 0-9.6z',
  arreter: 'M4 4h8v8h-8z',
  pause: 'M5 3.5v9M11 3.5v9',
  reprendre: 'M5 3l8 5l-8 5z',
  app: 'M2.5 3.5h11v9h-11zM2.5 6h11M4.5 4.75h.01M6 4.75h.01',
  carnet: 'M3.5 2.5h8l1.5 1.5v9.5h-9.5zM6 6h4.5M6 8.5h4.5M6 11h3',
  reglages: 'M2.5 4.5h11M2.5 11.5h11M6 2.5v4M10 9.5v4',
  voix: 'M2.5 6h2.5l3.5-3v10l-3.5-3h-2.5zM11 5.5q1.5 2.5 0 5M12.8 4q2.6 4 0 8',
  texte: 'M2.5 4h11M2.5 7h11M2.5 10h7',
  discret: 'M8 5.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 1 0 0-5z',
  deplier: 'M3 6l5-3l5 3M3 10l5 3l5-3',
  quitter: 'M4 4l8 8M12 4l-8 8',
};

function icon(name: string): SVGSVGElement {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', '0 0 16 16');
  svg.setAttribute('aria-hidden', 'true');
  svg.classList.add('kmenu-icon');
  const path = document.createElementNS(SVG, 'path');
  path.setAttribute('d', ICONS[name] ?? '');
  svg.append(path);
  return svg;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Ce qui fixe les libellés du menu. */
export interface MenuContext {
  recording: boolean;
  /** Réunion en pause, ou écoute en pause hors réunion. */
  paused: boolean;
  compact: boolean;
  /** Kobi répond à voix haute dans le contexte courant (KOB-142). */
  voice: boolean;
}

interface Entry {
  id: MenuId;
  label: string;
  icon: string;
  hint?: string;
  danger?: boolean;
}

/** Entrées et libellés selon l'état ; `null` : séparateur. */
export function entries(c: MenuContext): Array<Entry | null> {
  return [
    { id: 'parler', label: 'Parler à Kobi', icon: 'parler', hint: 'Ctrl Maj Espace' },
    // La bulle d'écriture (ecrire.ts, KOB-147) s'ouvre sur l'événement `kobi:menu`.
    { id: 'ecrire', label: 'Écrire à Kobi', icon: 'ecrire', hint: 'Ctrl Alt K' },
    c.recording
      ? { id: 'enregistrer', label: 'Arrêter l’enregistrement', icon: 'arreter', hint: 'Ctrl Maj R' }
      : { id: 'enregistrer', label: 'Enregistrer la réunion', icon: 'enregistrer', hint: 'Ctrl Maj R' },
    c.recording
      ? { id: 'pause', label: c.paused ? 'Reprendre l’enregistrement' : 'Pause de l’enregistrement', icon: c.paused ? 'reprendre' : 'pause' }
      : { id: 'pause', label: c.paused ? 'Reprendre l’écoute' : 'Pause de l’écoute', icon: c.paused ? 'reprendre' : 'pause' },
    null,
    // L'app Kobi (KOB-159) : section par défaut, Carnet, Réglages.
    { id: 'app', label: 'Ouvrir Kobi', icon: 'app' },
    { id: 'carnet', label: 'Ouvrir le Carnet', icon: 'carnet' },
    { id: 'reglages', label: 'Réglages', icon: 'reglages' },
    c.voice ? { id: 'voix', label: 'Répondre en texte seul', icon: 'texte' } : { id: 'voix', label: 'Répondre à voix haute', icon: 'voix' },
    c.compact ? { id: 'discret', label: 'Déplier Kobi', icon: 'deplier' } : { id: 'discret', label: 'Mode discret', icon: 'discret' },
    null,
    { id: 'quitter', label: 'Quitter Kobi', icon: 'quitter', danger: true },
  ];
}

/** Zones où le navigateur garde sa sélection et son menu (champ de saisie). */
function editable(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('input, textarea, [contenteditable="true"]') !== null;
}

/**
 * Menu du clic droit et comportements de base de la webview retirés
 * (menu contextuel, sélection, glisser d'images), sauf dans les champs de
 * saisie, qui gardent copier-coller.
 */
export class KobiMenu {
  readonly root: HTMLElement;
  private readonly list: HTMLElement;
  private readonly heading: HTMLElement;
  private readonly toast: HTMLElement;
  private readonly micOff: HTMLElement;
  private confirm: HTMLElement | null = null;
  private items: HTMLButtonElement[] = [];
  private returnFocus: HTMLElement | null = null;
  private listeningPaused = false;
  private toastTimer = 0;
  private closing = 0;
  private readonly cleanups: Array<() => void> = [];

  constructor(private readonly stage: Stage) {
    this.root = el('div', 'kmenu');
    this.root.hidden = true;
    this.root.setAttribute('role', 'menu');
    this.root.setAttribute('aria-label', 'Menu de Kobi');
    this.root.setAttribute('aria-orientation', 'vertical');
    this.heading = el('div', 'kmenu-head');
    this.heading.setAttribute('aria-hidden', 'true');
    this.list = el('div', 'kmenu-list');
    this.root.append(this.heading, this.list);

    this.toast = el('div', 'kmenu-toast');
    // Annoncé sans rôle `status` : la scène garde la seule région d'état.
    this.toast.setAttribute('aria-live', 'polite');
    this.toast.hidden = true;

    this.micOff = el('div', 'kmenu-micoff');
    this.micOff.append(el('span', 'kmenu-micoff-dot'), el('span', '', 'micro coupé'));
    this.micOff.hidden = true;

    stage.root.append(this.root, this.toast, this.micOff);
    stage.root.classList.add('no-select');

    const on = <K extends keyof DocumentEventMap>(type: K, fn: (e: DocumentEventMap[K]) => void, capture = false): void => {
      document.addEventListener(type, fn, capture);
      this.cleanups.push(() => document.removeEventListener(type, fn, capture));
    };
    on('contextmenu', (e) => {
      if (editable(e.target)) return;
      e.preventDefault();
      if (this.confirm) return;
      // Touche Menu : pas de position de souris, le menu s'ouvre sur Kobi.
      const fromKeyboard = e.button !== 2 || (e.clientX === 0 && e.clientY === 0);
      if (fromKeyboard) this.openAtKobi();
      else this.open(e.clientX, e.clientY);
    });
    on('selectstart', (e) => {
      if (!editable(e.target)) e.preventDefault();
    });
    on('dragstart', (e) => {
      if (!editable(e.target)) e.preventDefault();
    });
    on('keydown', (e) => {
      if (this.confirm || !this.root.hidden) return;
      if (e.key === 'F10' && e.shiftKey && !editable(e.target)) {
        e.preventDefault();
        this.openAtKobi();
      }
    });
    on(
      'pointerdown',
      (e) => {
        if (!this.root.hidden && !this.root.contains(e.target as Node)) this.close(false);
      },
      true,
    );
    const onBlur = (): void => this.close(false);
    window.addEventListener('blur', onBlur);
    this.cleanups.push(() => window.removeEventListener('blur', onBlur));

    this.root.addEventListener('keydown', (e) => this.onKey(e));
    this.followShell();
  }

  get isOpen(): boolean {
    return !this.root.hidden;
  }

  /** Réponse à voix haute : `data-voix` posé par la coque (`native.ts`). */
  private get voice(): boolean {
    const v = this.stage.root.dataset.voix;
    return v === 'active' || v === 'parle';
  }

  /** État courant des libellés. */
  context(): MenuContext {
    const meeting = this.stage.currentMeeting;
    return {
      recording: meeting !== null,
      paused: meeting ? Boolean(meeting.paused) : this.listeningPaused,
      compact: this.stage.isCompact,
      voice: this.voice,
    };
  }

  /** Ouvre le menu au point donné (coordonnées de la fenêtre), recalé dans la fenêtre. */
  open(x: number, y: number): void {
    window.clearTimeout(this.closing);
    this.returnFocus = document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : null;
    this.render();
    this.root.hidden = false;
    this.root.classList.remove('closing');
    const margin = 6;
    const { width, height } = this.root.getBoundingClientRect();
    const left = Math.max(margin, Math.min(x, window.innerWidth - width - margin));
    const top = Math.max(margin, Math.min(y, window.innerHeight - height - margin));
    this.root.style.left = `${left}px`;
    this.root.style.top = `${top}px`;
    // Le menu jaillit du point cliqué.
    this.root.style.setProperty('--ox', `${x - left}px`);
    this.root.style.setProperty('--oy', `${y - top}px`);
    this.root.classList.remove('opening');
    void this.root.offsetWidth;
    this.root.classList.add('opening');
    this.stage.root.classList.add('menu-open');
    this.items[0]?.focus({ preventScroll: true });
  }

  /** Ouvre le menu sur la mascotte (clavier). */
  openAtKobi(): void {
    const target = this.stage.isCompact ? this.stage.root.querySelector('.dot') : this.stage.kobi.root;
    const box = target?.getBoundingClientRect();
    if (!box || box.width === 0) this.open(window.innerWidth / 2, window.innerHeight / 3);
    else this.open(box.left + box.width / 2, box.top + box.height / 3);
  }

  close(restoreFocus = true): void {
    if (this.root.hidden) return;
    this.stage.root.classList.remove('menu-open');
    this.root.classList.remove('opening');
    this.root.classList.add('closing');
    window.clearTimeout(this.closing);
    const reduced = this.stage.root.classList.contains('reduced-motion');
    this.closing = window.setTimeout(() => {
      this.root.hidden = true;
      this.root.classList.remove('closing');
    }, reduced ? 0 : 140);
    if (restoreFocus) this.returnFocus?.focus({ preventScroll: true });
    this.returnFocus = null;
  }

  /** Exécute une entrée, comme un clic dessus. */
  async choose(id: MenuId): Promise<void> {
    // L'entrée choisie s'allume pendant que le menu se replie.
    this.items.find((b) => b.dataset.id === id)?.classList.add('chosen');
    this.close();
    window.dispatchEvent(new CustomEvent<MenuId>('kobi:menu', { detail: id }));
    const api = tauri();
    if (!api) {
      this.simulate(id);
      return;
    }
    // Voix : l'événement `voix` de la coque met `data-voix` à jour, peut-être
    // avant la fin de la commande ; le message part de l'état d'avant.
    const voix = this.voice;
    try {
      await api.core.invoke('menu_kobi', { entree: id });
      if (id === 'voix') this.say(voix ? 'Je réponds par écrit.' : 'Je réponds à voix haute.');
    } catch (refus) {
      this.say(typeof refus === 'string' && refus.includes('transition refusée') ? 'Pas maintenant : Kobi est occupé.' : String(refus));
    }
  }

  destroy(): void {
    this.cleanups.forEach((fn) => fn());
    this.root.remove();
    this.toast.remove();
    this.micOff.remove();
    this.confirm?.remove();
    this.stage.root.classList.remove('no-select', 'menu-open');
  }

  private render(): void {
    const c = this.context();
    this.heading.replaceChildren(el('span', 'kmenu-brand', 'kobi'), el('span', 'kmenu-state', c.recording ? (c.paused ? 'réunion en pause' : '● réunion en cours') : c.paused ? 'écoute en pause' : 'à ton écoute'));
    this.heading.classList.toggle('recording', c.recording && !c.paused);
    this.list.replaceChildren();
    this.items = [];
    let index = 0;
    for (const entry of entries(c)) {
      if (!entry) {
        const sep = el('div', 'kmenu-sep');
        sep.setAttribute('role', 'separator');
        this.list.append(sep);
        continue;
      }
      const b = el('button', 'kmenu-item');
      b.type = 'button';
      b.setAttribute('role', 'menuitem');
      b.tabIndex = -1;
      b.dataset.id = entry.id;
      b.classList.toggle('danger', Boolean(entry.danger));
      b.style.setProperty('--i', String(index++));
      b.append(icon(entry.icon), el('span', 'kmenu-label', entry.label));
      if (entry.hint) {
        const hint = el('kbd', 'kmenu-hint', entry.hint);
        hint.setAttribute('aria-hidden', 'true');
        b.append(hint);
        b.setAttribute('aria-keyshortcuts', entry.hint.replace('Maj', 'Shift').replace('Espace', 'Space').replace(/ /g, '+'));
      }
      b.addEventListener('click', () => void this.choose(entry.id));
      b.addEventListener('pointermove', () => {
        if (document.activeElement !== b) b.focus({ preventScroll: true });
      });
      this.items.push(b);
      this.list.append(b);
    }
  }

  private onKey(e: KeyboardEvent): void {
    const i = this.items.indexOf(document.activeElement as HTMLButtonElement);
    const n = this.items.length;
    const go = (j: number): void => {
      e.preventDefault();
      this.items[(j + n) % n]?.focus({ preventScroll: true });
    };
    switch (e.key) {
      case 'ArrowDown':
        return go(i + 1);
      case 'ArrowUp':
        return go(i < 0 ? n - 1 : i - 1);
      case 'Home':
        return go(0);
      case 'End':
        return go(n - 1);
      case 'Escape':
        // Fermer le menu n'interrompt pas la réponse en cours (Échap, KOB-133).
        e.preventDefault();
        e.stopPropagation();
        return this.close();
      case 'Tab':
        e.preventDefault();
        return this.close();
      case 'ContextMenu':
        e.preventDefault();
        return this.close();
      case 'Enter':
      case ' ':
        if (i >= 0) {
          e.preventDefault();
          void this.choose(this.items[i]?.dataset.id as MenuId);
        }
        return;
    }
    // Première lettre : l'entrée suivante qui commence par elle.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const key = e.key.toLocaleLowerCase('fr');
      for (let k = 1; k <= n; k++) {
        const b = this.items[(i + k) % n];
        if (b?.textContent?.trim().toLocaleLowerCase('fr').startsWith(key)) return go((i + k) % n);
      }
    }
  }

  /** Événements de la coque : entrées venues de la barre système ou du clic droit. */
  private followShell(): void {
    const api = tauri();
    if (!api) return;
    void api.event.listen<string>('kobi://menu', (e) => this.afterShell(e.payload as MenuId | 'quitter-confirme'));
    void api.event.listen<boolean>('ecoute', (e) => this.setListeningPaused(Boolean(e.payload)));
    api.core.invoke<boolean>('etat_ecoute').then(
      (p) => this.setListeningPaused(Boolean(p)),
      () => {},
    );
  }

  /** Ce que l'interface fait après la coque. */
  private afterShell(id: MenuId | 'quitter-confirme'): void {
    switch (id) {
      case 'parler':
        this.stage.setCompact(false);
        break;
      case 'discret':
        this.stage.setCompact(!this.stage.isCompact);
        break;
      // Carnet et Réglages : la coque ouvre leur fenêtre (KOB-140).
      case 'quitter-confirme':
        if (this.stage.currentMeeting) this.say('J’écris le compte rendu, puis je ferme.');
        break;
      case 'quitter':
        // La coque ne ferme pas pendant une réunion : elle attend la confirmation.
        if (this.stage.currentMeeting) this.askQuit();
        break;
    }
  }

  /** Navigateur et tests : l'interface joue le rôle de la coque. */
  private simulate(id: MenuId): void {
    const meeting = this.stage.currentMeeting;
    switch (id) {
      case 'parler':
        this.stage.setCompact(false);
        this.stage.setState('listen');
        break;
      case 'enregistrer':
        if (meeting) this.stage.endMeeting();
        else this.stage.startMeeting({ startedAt: Date.now() }, { consent: false });
        break;
      case 'pause':
        if (meeting) this.stage.updateMeeting({ paused: !meeting.paused });
        else this.setListeningPaused(!this.listeningPaused);
        break;
      case 'quitter':
        if (meeting) this.askQuit();
        else window.dispatchEvent(new CustomEvent('kobi:quitter'));
        break;
      case 'app':
        this.say('L’app Kobi s’ouvre depuis Kobi.');
        break;
      case 'carnet':
        this.say('Le Carnet s’ouvre depuis Kobi.');
        break;
      case 'reglages':
        this.say('Les réglages s’ouvrent depuis Kobi.');
        break;
      case 'voix':
        this.stage.root.dataset.voix = this.voice ? 'texte' : 'active';
        this.say(this.voice ? 'Je réponds à voix haute.' : 'Je réponds par écrit.');
        break;
      default:
        this.afterShell(id);
    }
  }

  private setListeningPaused(paused: boolean): void {
    this.listeningPaused = paused;
    this.micOff.hidden = !paused || !this.toast.hidden;
    this.stage.root.dataset.ecoute = paused ? 'pause' : 'active';
  }

  /** Message court sous le menu, 2,5 s. */
  private say(text: string): void {
    this.toast.textContent = text;
    this.toast.hidden = false;
    // Une seule chose à la fois autour de Kobi : le message passe devant « micro coupé ».
    this.micOff.hidden = true;
    this.toast.classList.remove('show');
    void this.toast.offsetWidth;
    this.toast.classList.add('show');
    window.clearTimeout(this.toastTimer);
    this.toastTimer = window.setTimeout(() => {
      this.toast.hidden = true;
      this.micOff.hidden = !this.listeningPaused;
    }, 2500);
  }

  /** Quitter pendant une réunion : Kobi demande s'il faut l'arrêter. */
  private askQuit(): void {
    if (this.confirm) return;
    this.stage.setCompact(false);
    const card = el('div', 'kmenu-confirm');
    card.setAttribute('role', 'alertdialog');
    card.setAttribute('aria-modal', 'true');
    const title = el('p', 'kmenu-confirm-title', 'Une réunion est en cours.');
    title.id = 'kmenu-confirm-title';
    const text = el('p', 'kmenu-confirm-text', 'L’arrêter, écrire le compte rendu et quitter ?');
    text.id = 'kmenu-confirm-text';
    card.setAttribute('aria-labelledby', title.id);
    card.setAttribute('aria-describedby', text.id);
    const stop = el('button', 'kmenu-btn kmenu-btn-danger', 'Arrêter et quitter');
    const cancel = el('button', 'kmenu-btn', 'Annuler');
    stop.type = cancel.type = 'button';
    const buttons = el('div', 'kmenu-confirm-buttons');
    buttons.append(cancel, stop);
    card.append(title, text, buttons);
    const done = (): void => {
      card.remove();
      this.confirm = null;
    };
    cancel.addEventListener('click', done);
    stop.addEventListener('click', () => {
      done();
      const api = tauri();
      if (api) void api.core.invoke('menu_kobi', { entree: 'quitter-confirme' });
      else {
        this.stage.endMeeting();
        window.dispatchEvent(new CustomEvent('kobi:quitter'));
      }
    });
    card.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        done();
      } else if (e.key === 'Tab') {
        // Le focus reste dans la carte.
        e.preventDefault();
        (document.activeElement === stop ? cancel : stop).focus();
      }
    });
    this.confirm = card;
    this.stage.root.append(card);
    cancel.focus({ preventScroll: true });
  }
}
