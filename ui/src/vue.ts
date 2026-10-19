// Vue agrandie de la conversation (KOB-169) : quand il faut lire, un panneau
// large et lisible s'ouvre à côté de Kobi (ou au centre de l'écran, ou sur la
// moitié droite, selon le réglage). La fenêtre de Kobi s'agrandit pour le
// contenir (`agrandir_kobi`) et reste transparente ; Kobi ne bouge pas et
// continue de vivre. Pas de deuxième fenêtre.
//
// Le panneau montre les échanges récents et la réponse en flux, mise en forme
// (listes, code), avec un défilement qui suit le flux et les étapes de
// KOB-152 repliées sous chaque réponse.
//
// Il s'ouvre tout seul pour une réponse qui dépasse la bulle (réglable), ou à
// la demande : clic sur la bulle, « Kobi, montre en grand », raccourci,
// `kobi view open` (événement `vue` de la coque). Il se range par Échap, par
// un clic hors du panneau, ou seul une fois la réponse finie (délai réglable ;
// la souris dessus le retient).

import { etapeLigne, type Etape } from './activite';
import { affichage, suivreAffichage, TAILLES, TEXTES, type Affichage } from './affichage';
import type { Bulle } from './conversation';
import { boutonCopierDedans } from './copie';
import { Defilement } from './defilement';
import { Markdown } from './markdown';
import { tauri } from './native';
import type { Boite, Stage } from './stage';
import type { MascotState } from './states';

/** États où Kobi est encore pris par l'échange : la vue ne se range pas seule. */
const ACTIF: ReadonlySet<MascotState> = new Set(['listen', 'think', 'answer', 'work']);
const MARGE = 16;

interface Disposition {
  kobi: Boite;
  panneau: Boite;
}

/** Un échange affiché : question, réponse mise en forme, étapes. */
interface Echange {
  node: HTMLElement;
  reponse: HTMLElement;
  md: Markdown;
  texte: string;
}

/**
 * La réponse en flux et celle de l'historique sont-elles la même ? La coque
 * publie la réponse finie rognée (espaces de fin) et la question n'arrive pas
 * toujours avant le flux (KOB-177) : même question, ou à défaut l'une prolonge
 * l'autre.
 */
function memeEchange(d: { question: string; reponse: string }, b: { question: string; reponse: string }): boolean {
  if (d.question !== '') return b.question === d.question;
  const x = d.reponse.trim();
  const y = b.reponse.trim();
  return x !== '' && y !== '' && (x.startsWith(y) || y.startsWith(x));
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Deux réponses à moins de cet écart font une rafale : rien ne s'ouvre seul (KOB-238). */
const RAFALE_MS = 10_000;

export class Vue {
  readonly root: HTMLElement;
  private readonly liste: HTMLElement;
  private readonly corps: HTMLElement;
  private readonly defile: Defilement;
  private reglages: Affichage = affichage();
  private ouverte = false;
  private enCours: Promise<void> | null = null;
  private placee = false;
  private bulles: Bulle[] = [];
  private readonly echanges = new Map<string, Echange>();
  /** Réponse en flux (la coque l'envoie avant de publier l'historique). */
  private direct: { question: string; reponse: string } | null = null;
  private timer = 0;
  private survol = false;
  /** Vérification du débordement déjà prévue à la prochaine image. */
  private verifie = false;
  /** Rangée pendant cette réponse : elle ne se rouvre pas seule avant la suivante. */
  private rangeeIci = false;
  /**
   * Ouverture seule déjà faite dans cette conversation : pas une seconde
   * sans que l'utilisateur la demande (KOB-238). Remis à zéro quand la
   * coque clôt la conversation (`fin_conversation`).
   */
  private ouverteSeule = false;
  /** Moment de la réponse précédente : deux réponses rapprochées font une rafale. */
  private reponsePrecedente = -Infinity;
  private derniereReponse = -Infinity;
  private etatPrecedent: MascotState | null = null;

  constructor(private readonly stage: Stage) {
    this.root = el('section', 'lecture');
    this.root.setAttribute('role', 'dialog');
    this.root.setAttribute('aria-label', 'Conversation avec Kobi');
    this.root.hidden = true;
    const tete = el('div', 'lecture-tete');
    const fermer = el('button', 'lecture-fermer', '×');
    fermer.type = 'button';
    fermer.setAttribute('aria-label', 'Ranger la conversation');
    fermer.addEventListener('click', () => void this.fermer('bouton'));
    tete.append(el('span', 'lecture-titre', 'Conversation'), fermer);
    this.corps = el('div', 'lecture-corps defile');
    this.liste = el('div', 'lecture-liste');
    this.corps.append(this.liste);
    this.root.append(tete, this.corps);
    stage.root.append(this.root);
    this.defile = new Defilement(this.corps, this.liste);

    this.root.addEventListener('pointerenter', () => (this.survol = true));
    this.root.addEventListener('pointerleave', () => {
      this.survol = false;
      this.armer();
    });
    // Un clic hors du panneau (zone transparente de la fenêtre agrandie) le range ;
    // un clic sur Kobi ou sur ses bulles, non.
    stage.root.addEventListener('pointerdown', (e) => {
      const t = e.target as Element | null;
      if (!this.ouverte || t?.closest('.lecture, .scene, .dot')) return;
      void this.fermer('clic à côté');
    });
    // Échap range la vue avant tout le reste (historique, interruption).
    window.addEventListener(
      'keydown',
      (e) => {
        if (e.key !== 'Escape' || !this.ouverte) return;
        e.preventDefault();
        e.stopImmediatePropagation();
        void this.fermer('Échap');
      },
      true,
    );
    void tauri()?.event.listen<{ action?: string }>('vue', (e) => this.commande(e.payload?.action));
    window.addEventListener('kobi:vue', (e) => this.commande((e as CustomEvent<string>).detail));

    this.appliquerReglages(this.reglages);
    suivreAffichage((a) => this.appliquerReglages(a));

    // Conversation close par la coque (pas chaque retour en veille, qui suit
    // chaque réponse) : la prochaine pourra s'ouvrir seule une fois.
    void tauri()?.event.listen('fin_conversation', () => {
      this.ouverteSeule = false;
    });

    const previous = stage.onRender;
    stage.onRender = (state, data) => {
      previous?.(state, data);
      this.surRendu(state, data.question, data.answer);
    };
    // La frappe allonge la bulle après le rendu : tant que Kobi répond, on
    // regarde si elle déborde (ouverture automatique « réponses longues »).
    new MutationObserver(() => this.verifierDebordement()).observe(stage.root.querySelector('.bubbles') ?? stage.root, {
      childList: true,
      subtree: true,
      characterData: true,
    });
  }

  get isOpen(): boolean {
    return this.ouverte;
  }

  /** Historique publié par la coque. */
  setBulles(bulles: readonly Bulle[]): void {
    this.bulles = [...bulles];
    // L'historique a rattrapé le flux (tour fini, ou au moins aussi long) : il fait foi.
    const d = this.direct;
    if (d && bulles.some((b) => memeEchange(d, b) && (b.statut === 'fini' || b.statut === 'erreur' || b.reponse.trim().length >= d.reponse.trim().length))) this.direct = null;
    if (this.ouverte) this.rendre();
  }

  commande(action: string | undefined): void {
    if (action === 'fermer') void this.fermer('demande');
    else if (action === 'basculer') void (this.ouverte ? this.fermer('demande') : this.ouvrir('demande'));
    else void this.ouvrir('demande');
  }

  /**
   * L'ouverture seule est permise : pas pendant une rafale (deux réponses à
   * moins de 10 s), et une fois au plus par conversation (KOB-238).
   */
  private ouvertureSeulePermise(): boolean {
    return !this.ouverteSeule && this.derniereReponse - this.reponsePrecedente > RAFALE_MS;
  }

  /** Ouvre la vue (ou la garde ouverte) : la fenêtre s'agrandit d'abord. */
  async ouvrir(raison = 'clic'): Promise<void> {
    window.clearTimeout(this.timer);
    if (this.ouverte) return;
    if (raison === 'auto' || raison === 'réponse longue') {
      if (!this.ouvertureSeulePermise()) return;
      this.ouverteSeule = true;
    }
    await this.enCours;
    this.ouverte = true;
    this.stage.root.classList.add('vue-ouverte');
    this.rendre();
    this.enCours = this.agrandir(raison);
    await this.enCours;
    this.enCours = null;
    this.root.hidden = false;
    this.root.dataset.etat = 'ouverte';
    this.animer(true);
    this.defile.suivreBas();
    this.armer();
  }

  async fermer(raison = 'demande'): Promise<void> {
    if (!this.ouverte) return;
    await this.enCours;
    this.ouverte = false;
    this.rangeeIci = ACTIF.has(this.stage.currentState);
    window.clearTimeout(this.timer);
    this.root.dataset.etat = 'fermeture';
    await this.animer(false);
    this.root.hidden = true;
    this.root.dataset.etat = 'rangee';
    this.stage.root.classList.remove('vue-ouverte');
    this.enCours = this.rendreFenetre(raison);
    await this.enCours;
    this.enCours = null;
  }

  private appliquerReglages(a: Affichage): void {
    this.reglages = a;
    const r = this.stage.root;
    r.classList.toggle('affichage-simple', a.mode === 'simple');
    r.classList.toggle('affichage-detaille', a.mode === 'detaille');
    r.style.setProperty('--vue-texte', `${TEXTES[a.texte]}px`);
    this.root.dataset.position = a.position;
    this.root.dataset.taille = a.taille;
    // Vue ouverte : la nouvelle taille ou la nouvelle place s'applique tout de suite.
    if (this.ouverte && !this.enCours) {
      this.enCours = this.agrandir('réglage').finally(() => (this.enCours = null));
    }
    this.armer();
  }

  private surRendu(state: MascotState, question: string | undefined, reponse: string | undefined): void {
    if (state === 'answer' && this.etatPrecedent !== 'answer') {
      this.reponsePrecedente = this.derniereReponse;
      this.derniereReponse = performance.now();
    }
    this.etatPrecedent = state;
    if (state === 'answer' || state === 'listen' || state === 'think') {
      if (question) this.direct = { question, reponse: reponse ?? '' };
      else if (this.direct && reponse !== undefined) this.direct.reponse = reponse;
      else if (reponse) this.direct = { question: '', reponse };
    }
    if (this.ouverte) this.rendre();
    if (!ACTIF.has(state)) this.rangeeIci = false;
    if (state === 'answer' && !this.ouverte && !this.rangeeIci) {
      const auto = this.reglages.auto;
      if (auto === 'toujours' && (reponse ?? '') !== '') void this.ouvrir('auto');
      // « Réponses longues » : dès que la bulle ne peut plus tout montrer.
      else this.verifierDebordement();
    }
    this.armer();
  }

  private verifierDebordement(): void {
    if (this.ouverte || this.verifie || this.rangeeIci || this.reglages.auto !== 'longues' || this.stage.currentState !== 'answer') return;
    this.verifie = true;
    requestAnimationFrame(() => {
      this.verifie = false;
      if (!this.ouverte && this.stage.currentState === 'answer' && this.deborde()) void this.ouvrir('réponse longue');
    });
  }

  /** La réponse dépasse ce que la bulle montre. */
  private deborde(): boolean {
    const z = this.stage.root.querySelector<HTMLElement>('[data-card=answer] .defile');
    return z !== null && z.scrollHeight > z.clientHeight + 4;
  }

  /** Se range seule une fois la réponse finie, après le délai réglé (la souris dessus la retient). */
  private armer(): void {
    window.clearTimeout(this.timer);
    if (!this.ouverte || this.reglages.delai === 0 || ACTIF.has(this.stage.currentState)) return;
    this.timer = window.setTimeout(() => {
      if (this.survol || ACTIF.has(this.stage.currentState)) this.armer();
      else void this.fermer('délai');
    }, this.reglages.delai * 1000);
  }

  /** Échanges : l'historique, puis la réponse en flux si elle n'y est pas encore. */
  private rendre(): void {
    const items: Array<{ cle: string; question: string; reponse: string; etapes: readonly Etape[]; fini: boolean }> = this.bulles.slice(-12).map((b) => ({
      cle: `t${b.tour}`,
      question: b.question,
      reponse: b.reponse,
      etapes: b.etapes ?? [],
      fini: b.statut === 'fini' || b.statut === 'erreur',
    }));
    const d = this.direct;
    if (d) {
      // Un seul échange par tour : le flux complète celui de l'historique
      // (jamais un second, KOB-177).
      const meme = items.find((i) => memeEchange(d, i));
      if (meme) {
        if (!meme.fini && d.reponse.length > meme.reponse.length) meme.reponse = d.reponse;
      } else items.push({ cle: 'direct', question: d.question, reponse: d.reponse, etapes: [], fini: false });
    }
    const vus = new Set<string>();
    for (const it of items) {
      vus.add(it.cle);
      let e = this.echanges.get(it.cle);
      if (!e) {
        const node = el('article', 'vex');
        const q = el('div', 'vex-q', it.question);
        const reponse = el('div', 'vex-a md');
        // Une fois la réponse finie (vue.css) ; elle part en entier, markdown compris.
        const cle = it.cle;
        node.append(q, reponse, boutonCopierDedans('reponse', () => this.echanges.get(cle)?.texte ?? '', 'vex-copier'));
        this.liste.append(node);
        e = { node, reponse, md: new Markdown(reponse, () => !this.stage.root.classList.contains('reduced-motion')), texte: '' };
        this.echanges.set(it.cle, e);
      }
      e.node.querySelector('.vex-q')!.textContent = it.question;
      e.node.classList.toggle('vex-sans-question', it.question === '');
      e.node.dataset.fini = String(it.fini);
      if (!it.reponse.startsWith(e.texte)) {
        e.reponse.replaceChildren();
        e.md = new Markdown(e.reponse, () => false);
        e.texte = '';
      }
      if (it.reponse !== e.texte || it.fini) {
        e.md.feed(it.reponse, it.fini);
        e.texte = it.reponse;
      }
      this.etapes(e.node, it.etapes);
    }
    for (const [cle, e] of this.echanges) {
      if (!vus.has(cle)) {
        e.node.remove();
        this.echanges.delete(cle);
      }
    }
  }

  /** Étapes du tour (KOB-152), repliées sous la réponse. */
  private etapes(node: HTMLElement, etapes: readonly Etape[]): void {
    let box = node.querySelector<HTMLDetailsElement>('.vex-etapes');
    if (etapes.length === 0) {
      box?.remove();
      return;
    }
    if (!box) {
      box = el('details', 'vex-etapes');
      box.append(el('summary'), el('div', 'vex-etapes-liste'));
      node.append(box);
    }
    if (box.dataset.n === String(etapes.length)) return;
    box.dataset.n = String(etapes.length);
    box.querySelector('summary')!.textContent = etapes.length === 1 ? '1 étape' : `${etapes.length} étapes`;
    box.querySelector('.vex-etapes-liste')!.replaceChildren(...etapes.map((x) => etapeLigne(x)));
  }

  /** Demande la place à la coque et pose la scène et le panneau. */
  private async agrandir(raison: string): Promise<void> {
    const [largeur, hauteur] = TAILLES[this.reglages.taille];
    const api = tauri();
    if (!api) {
      // Hors de l'app (navigateur, tests sans coque) : le panneau se pose dans la fenêtre.
      this.poser({ x: MARGE, y: MARGE, largeur: Math.min(largeur, window.innerWidth * 0.55), hauteur: Math.min(hauteur, window.innerHeight - 2 * MARGE) });
      return;
    }
    const args = { largeur, hauteur, position: this.reglages.position, raison };
    try {
      const d = await api.core.invoke<Disposition | null>('agrandir_kobi', { grand: true, appliquer: false, ...args });
      if (!d) return;
      this.stage.freezeScale(true);
      this.stage.setPlacement(d.kobi);
      this.poser(d.panneau);
      this.placee = true;
      await api.core.invoke('agrandir_kobi', { grand: true, appliquer: true, ...args });
      await tailleAtteinte((w) => w >= d.kobi.x + d.kobi.largeur - 2);
    } catch {
      this.stage.setPlacement(null);
    } finally {
      this.stage.freezeScale(false);
    }
  }

  /** La fenêtre reprend sa taille ; la scène reste en place pendant ce temps. */
  private async rendreFenetre(raison: string): Promise<void> {
    const api = tauri();
    if (!api || !this.placee) return;
    this.placee = false;
    this.stage.freezeScale(true);
    const petite = this.stage.agrandie;
    try {
      await api.core.invoke('agrandir_kobi', { grand: false, appliquer: true, raison });
      // La scène ne se recale qu'une fois la fenêtre vraiment rendue à sa
      // taille (Windows peut mettre du temps à la redimensionner).
      if (petite) await tailleAtteinte((w) => w <= petite.largeur + 2);
    } catch {
      // Déjà à sa taille.
    } finally {
      this.stage.setPlacement(null);
      this.stage.freezeScale(false);
    }
  }

  /**
   * Pose le panneau dans la place donnée, rentré de son débord (anneau
   * acide tout autour, ombre en bas à droite, vue.css) : la coque agrandit la
   * fenêtre au ras du panneau, et un contour qui en sortait était coupé au
   * bord de la fenêtre transparente, coins compris (KOB-246).
   */
  private poser(p: Boite): void {
    const s = this.root.style;
    s.left = `${p.x + DEBORD.anneau}px`;
    s.top = `${p.y + DEBORD.anneau}px`;
    s.width = `${Math.max(0, p.largeur - 2 * DEBORD.anneau - DEBORD.ombre)}px`;
    s.height = `${Math.max(0, p.hauteur - 2 * DEBORD.anneau - DEBORD.ombre)}px`;
  }

  /** Arrivée et départ : il grandit un peu depuis Kobi ; « réduire les animations » : fondu court. */
  private animer(entree: boolean): Promise<void> {
    const reduit = this.stage.root.classList.contains('reduced-motion');
    const frames: Keyframe[] = reduit
      ? [{ opacity: 0 }, { opacity: 1 }]
      : [
          { opacity: 0, transform: 'translateY(14px) scale(.96)' },
          { opacity: 1, transform: 'none' },
        ];
    if (!entree) frames.reverse();
    const a = this.root.animate(frames, { duration: reduit ? 140 : 240, easing: entree ? 'cubic-bezier(.2,.8,.3,1)' : 'ease-in', fill: entree ? 'none' : 'forwards' });
    return a.finished.then(
      () => {
        if (!entree) a.cancel();
      },
      () => {},
    );
  }
}

/** Attend que la largeur de la fenêtre vérifie `ok` (au plus 3 s). */
/** Débord du contour du panneau hors de sa boîte (vue.css : `.lecture`). */
const DEBORD = { anneau: 3, ombre: 10 };

function tailleAtteinte(ok: (largeur: number) => boolean): Promise<void> {
  return new Promise((resolve) => {
    const debut = performance.now();
    const voir = (): void => {
      if (ok(window.innerWidth) || performance.now() - debut > 3_000) resolve();
      else requestAnimationFrame(voir);
    };
    voir();
  });
}
