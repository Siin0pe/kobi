// Carte de validation (KOB-62) à côté de Kobi : une seule carte pour tout ce
// qui écrit (fichier, commande, MCP, réglage, suppression). Contrat commun des
// permissions, section 4 : événement `carte` avec `type: "validation"`,
// réponse `decider_carte { id, accepte }`.
//
// Sobriété : la carte reste courte (résumé sur une ligne, déplié d'un clic,
// cible sur une ligne, compte à rebours, Valider / Refuser) ; « Détail dans Kobi » ouvre la section
// Activité de l'app, où le diff, la commande ou les paramètres se lisent en
// entier. Une seule carte à la fois, les suivantes attendent (badge « +N »).
//
// Valider / Refuser à la souris, Entrée / Échap au clavier (Entrée seulement
// si rien d'autre n'a le focus, KOB-215) ; « Ne plus demander » règle la
// politique `[permissions]` du genre, révocable dans Réglages › Permissions ; la voix répond
// côté coque, qui republie alors la carte avec son `statut`. Sans réponse à
// l'échéance (60 s), le serveur de permission refuse seul : la carte l'annonce
// et s'en va. Les boutons ne bougent jamais : seuls le pictogramme et la
// barre du compte à rebours vivent.
//
// Pendant qu'une carte attend, la scène porte `data-validation` (le genre),
// pour que la mascotte puisse y réagir.

import { tauri } from './native';
import type { Stage } from './stage';
import { cible, genre, type CarteValidation, type Statut } from './validation-detail';
import './styles/validation.css';

export { isValidation, type CarteValidation } from './validation-detail';

const ECHEANCE_MS = 60_000;
/** Dernières secondes : la barre rougit, le chiffre bat. */
const URGENT_MS = 10_000;
/** Temps d'affichage de l'issue avant que la carte s'en aille. */
const ISSUE_MS = 1400;
const SORTIE_MS = 420;

const ISSUES: Record<Statut, { titre: string; meta: string }> = {
  acceptee: { titre: 'Validé.', meta: 'Kobi s’en occupe.' },
  refusee: { titre: 'Refusé.', meta: 'Kobi n’y touche pas.' },
  expiree: { titre: 'Pas de réponse : refusé.', meta: 'Rien n’a été modifié.' },
};

/** Genres qu'une case « Ne plus demander » peut laisser passer (jamais une suppression ni un service). */
const TOUJOURS: Partial<Record<CarteValidation['genre'], string>> = {
  fichier: 'Ne plus demander pour écrire des fichiers',
  commande: 'Ne plus demander pour les commandes',
  reglage: 'Ne plus demander pour les réglages',
};

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

interface Ouverte {
  carte: CarteValidation;
  root: HTMLElement;
  echeance: number;
  timer: number;
  bar: HTMLElement;
  secs: HTMLElement;
  badge: HTMLElement;
  toujours: HTMLInputElement | null;
  statut: Statut | null;
}

/** Les cartes de validation d'une scène. */
export class Validations {
  private readonly scene: HTMLElement;
  /** Cartes en attente de leur tour, avec leur heure d'arrivée (l'échéance court déjà). */
  private readonly file: Array<{ carte: CarteValidation; recue: number }> = [];
  private courante: Ouverte | null = null;
  /** Cartes déjà tranchées : une republication tardive ne les rouvre pas. */
  private readonly closes = new Set<string>();

  constructor(private readonly stage: Stage) {
    this.scene = stage.root.querySelector<HTMLElement>('.scene') ?? stage.root;
    document.addEventListener('keydown', (e) => this.onKey(e), true);
  }

  private get reduced(): boolean {
    return this.stage.root.classList.contains('reduced-motion');
  }

  /** Carte publiée par la coque : nouvelle, ou tranchée ailleurs (voix, échéance). */
  show(carte: CarteValidation): void {
    if (carte.statut) {
      if (this.courante?.carte.id === carte.id) this.conclure(carte.statut);
      else {
        const i = this.file.findIndex((f) => f.carte.id === carte.id);
        if (i >= 0) this.file.splice(i, 1);
        this.closes.add(carte.id);
        this.majBadge();
      }
      return;
    }
    if (this.closes.has(carte.id) || this.courante?.carte.id === carte.id || this.file.some((f) => f.carte.id === carte.id)) return;
    if (this.courante) {
      this.file.push({ carte, recue: performance.now() });
      this.majBadge();
      return;
    }
    this.ouvrir(carte, performance.now());
  }

  /** Carte affichée (tests, captures). */
  get current(): CarteValidation | null {
    return this.courante?.carte ?? null;
  }

  /** Réponse de l'utilisateur (bouton, Entrée / Échap). */
  decide(accepte: boolean): void {
    const o = this.courante;
    if (!o || o.statut) return;
    const toujours = accepte && o.toujours?.checked === true;
    this.conclure(accepte ? 'acceptee' : 'refusee', toujours);
    const api = tauri();
    if (!api) return;
    // La politique d'abord : les actions suivantes du même genre passent seules.
    const regle = toujours
      ? api.core.invoke('regler_permission', { genre: o.carte.genre, sansDemander: true }).catch(() => {})
      : Promise.resolve();
    void regle.then(() =>
      api.core.invoke('decider_carte', { id: o.carte.id, accepte }).catch(() => {
        // Déjà expirée côté coque : Claude a reçu un refus.
        if (this.courante === o) this.afficherIssue(o, 'expiree');
      }),
    );
  }

  /** `recue` : arrivée de la carte ; le serveur refuse à `recue + echeance_ms`. */
  private ouvrir(carte: CarteValidation, recue: number): void {
    const g = genre(carte);
    const root = el('div', 'card kval');
    root.dataset.card = 'validation';
    root.dataset.genre = carte.genre;
    root.setAttribute('role', 'alertdialog');
    root.setAttribute('aria-labelledby', `kval-titre-${carte.id}`);
    root.setAttribute('aria-describedby', `kval-temps-${carte.id}`);
    root.tabIndex = -1;

    const head = el('div', 'kval-head');
    const badge = el('span', 'kval-badge');
    badge.hidden = true;
    head.append(el('span', 'kval-glyph', g.glyph), el('span', 'kicker', g.kicker), badge);
    // Une ligne ; un clic déplie le résumé entier. Coupé à l'écran seulement :
    // un lecteur d'écran lit toujours le texte complet.
    const titre = el('div', 'title', carte.resume);
    titre.id = `kval-titre-${carte.id}`;
    titre.title = carte.resume;
    titre.addEventListener('click', () => titre.classList.toggle('deplie'));
    root.append(head, titre);
    const ou = cible(carte.detail);
    if (ou && ou !== carte.resume) root.append(el('div', 'kval-cible', ou));

    const temps = el('div', 'kval-timer');
    temps.id = `kval-temps-${carte.id}`;
    const track = el('div', 'kval-track');
    const bar = el('span', 'kval-bar');
    track.append(bar);
    const secs = el('span', 'kval-secs');
    temps.append(track, secs);

    const valider = el('button', 'btn btn-primary', carte.genre === 'suppression' ? 'Supprimer' : 'Valider');
    valider.type = 'button';
    valider.setAttribute('aria-keyshortcuts', 'Enter');
    valider.addEventListener('click', () => this.decide(true));
    const refuser = el('button', 'btn', 'Refuser');
    refuser.type = 'button';
    refuser.setAttribute('aria-keyshortcuts', 'Escape');
    refuser.addEventListener('click', () => this.decide(false));
    const buttons = el('div', 'buttons');
    buttons.append(valider, refuser);
    // Le détail complet se lit dans l'app Kobi, section Activité.
    const detail = el('button', 'kval-lien', 'Détail dans Kobi');
    detail.type = 'button';
    detail.addEventListener('click', () => void tauri()?.core.invoke('ouvrir_app', { section: 'activite' }).catch(() => {}));
    root.append(temps, buttons);
    let toujours: HTMLInputElement | null = null;
    const libelle = TOUJOURS[carte.genre];
    if (libelle) {
      const ligne = el('label', 'kval-toujours');
      toujours = el('input');
      toujours.type = 'checkbox';
      ligne.append(toujours, el('span', undefined, libelle));
      root.append(ligne);
    }
    root.append(detail);

    const o: Ouverte = { carte, root, echeance: recue + (carte.echeance_ms ?? ECHEANCE_MS), timer: 0, bar, secs, badge, toujours, statut: null };
    this.courante = o;
    this.scene.append(root);
    this.scene.classList.add('validation-open');
    this.stage.root.dataset.validation = carte.genre;
    this.majBadge();
    this.tick(o);
    // Le focus vient sur la carte, sauf si l'on est en train d'écrire.
    const actif = document.activeElement;
    if (!actif?.closest('input, textarea, [contenteditable="true"]')) root.focus({ preventScroll: true });
  }

  /** Compte à rebours : une mise à jour par seconde, calée sur l'échéance. */
  private tick(o: Ouverte): void {
    if (this.courante !== o || o.statut) return;
    const reste = Math.max(0, o.echeance - performance.now());
    const total = o.carte.echeance_ms ?? ECHEANCE_MS;
    o.secs.textContent = `${Math.ceil(reste / 1000)} s`;
    o.bar.style.transform = `scaleX(${(reste / total).toFixed(4)})`;
    o.root.classList.toggle('urgent', reste <= URGENT_MS);
    if (reste <= 0) {
      // Le serveur de permission a refusé de son côté.
      this.conclure('expiree');
      return;
    }
    o.timer = window.setTimeout(() => this.tick(o), ((reste - 1) % 1000) + 1);
  }

  private conclure(statut: Statut, toujours = false): void {
    const o = this.courante;
    if (!o || o.statut) return;
    window.clearTimeout(o.timer);
    this.closes.add(o.carte.id);
    this.afficherIssue(o, statut, toujours);
  }

  /** L'issue remplace la question, puis la carte part (vers Kobi si validée). */
  private afficherIssue(o: Ouverte, statut: Statut, toujours = false): void {
    o.statut = statut;
    window.clearTimeout(o.timer);
    const issue = toujours ? { ...ISSUES[statut], meta: 'Kobi ne demandera plus (Réglages › Permissions).' } : ISSUES[statut];
    o.root.dataset.statut = statut;
    o.root.classList.remove('urgent');
    o.root.setAttribute('role', 'status');
    o.root.replaceChildren(el('div', 'kicker', genre(o.carte).kicker), el('div', 'title', issue.titre), el('div', 'meta', issue.meta));
    delete this.stage.root.dataset.validation;
    o.timer = window.setTimeout(() => this.fermer(o), this.reduced ? ISSUE_MS / 2 : ISSUE_MS);
  }

  private fermer(o: Ouverte): void {
    if (this.courante !== o) return;
    const fin = (): void => {
      o.root.remove();
      if (this.courante !== o) return;
      this.courante = null;
      // Les cartes dont l'échéance est passée pendant l'attente sont déjà refusées.
      let suivante = this.file.shift();
      while (suivante && performance.now() >= suivante.recue + (suivante.carte.echeance_ms ?? ECHEANCE_MS)) {
        this.closes.add(suivante.carte.id);
        suivante = this.file.shift();
      }
      if (suivante) this.ouvrir(suivante.carte, suivante.recue);
      else this.scene.classList.remove('validation-open');
    };
    if (this.reduced) {
      fin();
      return;
    }
    o.root.classList.add('leaving');
    window.setTimeout(fin, SORTIE_MS);
  }

  private majBadge(): void {
    const o = this.courante;
    if (!o) return;
    const n = this.file.length;
    o.badge.hidden = n === 0;
    o.badge.textContent = `+${n}`;
    o.badge.setAttribute('aria-label', `${n} autre${n > 1 ? 's' : ''} demande${n > 1 ? 's' : ''} en attente`);
  }

  /** Entrée valide, Échap refuse ; jamais pendant une saisie ni dans le menu. */
  private onKey(e: KeyboardEvent): void {
    const o = this.courante;
    if (!o || o.statut || e.isComposing) return;
    const cible = e.target instanceof Element ? e.target : null;
    if (cible?.closest('.kmenu, .kmenu-confirm, input, textarea, [contenteditable="true"]')) return;
    // Espace sur un bouton de la carte : son clic, sans ouvrir la bulle
    // d'écriture (qui s'ouvre à la première touche tapée).
    if (e.key === ' ' && cible instanceof HTMLButtonElement && o.root.contains(cible)) {
      e.stopPropagation();
      return;
    }
    // Entrée valide seulement si le focus est sur la carte ou nulle part :
    // jamais pour un autre bouton, lien ou « N étapes » de Kobi (KOB-215).
    const ailleurs = cible !== null && cible !== document.body && !o.root.contains(cible) && cible.closest('button, a, summary, select, [role=button], [tabindex]') !== null;
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      this.decide(false);
    } else if (e.key === 'Enter') {
      // Valider et Refuser gardent leur propre clic ; ailleurs (même sur
      // « Détail dans Kobi »), Entrée valide.
      if (ailleurs || (cible instanceof HTMLButtonElement && cible.closest('.kval .buttons'))) return;
      e.preventDefault();
      e.stopPropagation();
      this.decide(true);
    }
  }
}

const parScene = new WeakMap<Stage, Validations>();

/** Les cartes de validation de cette scène (créées au premier usage). */
export function validations(stage: Stage): Validations {
  let v = parScene.get(stage);
  if (!v) {
    v = new Validations(stage);
    parScene.set(stage, v);
  }
  return v;
}

/** Aiguillage depuis l'événement `carte`. */
export function showValidation(stage: Stage, carte: CarteValidation): void {
  validations(stage).show(carte);
}
