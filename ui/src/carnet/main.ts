// Le Carnet (KOB-140), section « Notes et réunions » de l'app Kobi
// (KOB-159) : relire réunions, conversations et rappels rangés par Kobi.
// Index à gauche (recherche, filtres, rappels, liste par jour), page à
// droite (compte rendu, transcription, notes, ou conversation). Toutes les
// données viennent des commandes `carnet_*` de kobi-app.

import './carnet.css';
import { animationsReduites, h, normaliser, surligner } from '../dom';
import { disponible, ecouter, message } from '../ipc';
import { reagir } from '../app/mini-kobi';
import { creerNotifieur } from '../notif';
import { copier, formatCopie, type Quoi } from '../copie';
import { boutonCopie, copierPuces, ELEMENT, ligneMd, menuCopier, PARTIE, partiesReunion, type QuandCopie } from './copie-reunion';
import { STOCKAGE } from '../reglages/stockage';
import { poserTitre } from '../titre';
import { carnet, type ConversationLue, type Genre, type Index, type ResumeReunion, type ReunionLue, type Trouve } from './donnees';
import { cleDuJour, duree, echeance, heure, horodatage, libelleDuJour, markdown } from './format';
import { surLangue, t } from '../i18n';

type Filtre = 'tout' | Genre;
type Onglet = 'compte-rendu' | 'transcription' | 'notes';
type Selection = { genre: Genre; id: string } | { panneau: 'stockage' } | null;
/** Où aller dans la page ouverte (résultat de recherche). */
interface Cible {
  onglet?: Onglet;
  start?: number | null;
  at_ms?: number;
}

/** Ce que l'app donne au Carnet. */
export interface OptionsCarnet {
  /** Ouvre un panneau de la section Réglages (« Stockage » du pied de page). */
  reglages?: (panneau: string) => void;
}

export interface VueCarnet {
  rafraichir(): Promise<void>;
  ouvrir(genre: Genre, id: string): Promise<void>;
  /** Ouvre une réunion sur son compte rendu (carte « Compte rendu prêt », KOB-74). */
  ouvrirReunion(nom: string): Promise<void>;
}

/** Monte le Carnet dans `hote` ; rend de quoi le rafraîchir et l'ouvrir sur un élément. */
export function monterCarnet(hote: HTMLElement, options: OptionsCarnet = {}): VueCarnet {
  const etat = {
    index: null as Index | null,
    filtre: 'tout' as Filtre,
    requete: '',
    resultats: null as Trouve[] | null,
    selection: null as Selection,
    onglet: 'compte-rendu' as Onglet,
  };

  // ----- Squelette -----

  const recherche = h('input', {
    type: 'search',
    class: 'champ',
    spellcheck: 'false',
    autocomplete: 'off',
  });
  const filtres = h('div', { class: 'filtres', role: 'group' });
  const rappels = h('section', { class: 'rappels' });
  const liste = h('nav', { class: 'liste' });
  const marque = h('h1', { class: 'marque-mot' });
  const libelleStockage = h('span', {});
  const racine = h('code', { class: 'racine' });
  const page = h('main', { class: 'page', 'aria-live': 'polite' });
  const app = h(
    'div',
    { class: 'carnet' },
    h(
      'aside',
      { class: 'index' },
      h('header', { class: 'marque' }, marque),
      h('div', { class: 'recherche' }, h('span', { class: 'loupe', 'aria-hidden': 'true' }, '?'), recherche),
      filtres,
      h('div', { class: 'defile' }, rappels, liste),
      h(
        'footer',
        { class: 'pied' },
        h('button', { type: 'button', class: 'bouton-pied', 'data-action': 'stockage', onclick: () => ouvrirStockage() }, h('span', { 'aria-hidden': 'true' }, '⚙'), libelleStockage),
        racine,
      ),
    ),
    page,
  );
  hote.append(app);
  const notifier = creerNotifieur(document.body);


  const FILTRES: Array<[Filtre, string]> = [
    ['tout', 'filtreTout'],
    ['reunion', 'filtreReunions'],
    ['conversation', 'filtreConversations'],
  ];

  /** Textes du squelette (redits au changement de langue). */
  function textesSquelette(): void {
    recherche.placeholder = t('carnet.main.recherchePlaceholder');
    recherche.setAttribute('aria-label', t('carnet.main.rechercheAria'));
    filtres.setAttribute('aria-label', t('carnet.main.filtresAria'));
    rappels.setAttribute('aria-label', t('carnet.main.rappelsAria'));
    liste.setAttribute('aria-label', t('carnet.main.listeAria'));
    marque.textContent = t('carnet.main.marque');
    marque.setAttribute('aria-label', t('carnet.main.marque'));
    libelleStockage.textContent = ` ${t('carnet.main.stockage')}`;
    for (const b of filtres.children) {
      const cle = FILTRES.find(([v]) => v === b.getAttribute('data-filtre'))?.[1];
      if (cle) b.textContent = t(`carnet.main.${cle}`);
    }
  }

  for (const [valeur] of FILTRES) {
    filtres.append(
      h(
        'button',
        {
          type: 'button',
          class: 'filtre',
          'data-filtre': valeur,
          'aria-pressed': String(valeur === etat.filtre),
          onclick: () => {
            etat.filtre = valeur;
            for (const b of filtres.children) b.setAttribute('aria-pressed', String(b.getAttribute('data-filtre') === valeur));
            dessinerListe();
          },
        },
      ),
    );
  }
  textesSquelette();

  // ----- Index -----

  function titreReunion(r: { titre: string | null; debut_ms: number | null; date: string | null }): string {
    if (r.titre) return r.titre;
    const cle = r.debut_ms !== null ? cleDuJour(r.debut_ms) : r.date;
    return cle ? t('carnet.main.reunionDu', { jour: libelleDuJour(cle, Date.now(), true) }) : t('carnet.main.reunion');
  }

  function titreConversation(apercu: string, titre?: string | null): string {
    if (titre) return titre;
    const a = apercu.trim();
    return a ? (a.length > 70 ? `${a.slice(0, 68)}…` : a) : t('carnet.main.conversation');
  }

  interface Entree {
    genre: Genre;
    id: string;
    jour: string;
    ms: number;
    titre: string;
    meta: string;
    apercu: string;
    enCours: boolean;
  }

  /**
   * Durée d'une réunion : celle de sa transcription, ou, en cours, depuis son
   * début (KOB-251 : la liste ne se relit qu'au retour sur la fenêtre).
   */
  function dureeReunion(r: ResumeReunion): string | null {
    if (r.en_cours && r.debut_ms !== null) return duree(Math.max(0, (Date.now() - r.debut_ms) / 1000));
    return r.duree_s > 0 ? duree(r.duree_s) : null;
  }

  /** « Réunion · aujourd'hui · 06:57 · 12 min » (en-tête de la vue). */
  function surtitreReunion(r: ResumeReunion): string {
    const jour = r.debut_ms !== null ? cleDuJour(r.debut_ms) : r.date;
    return [
      t('carnet.main.reunion'),
      jour ? libelleDuJour(jour) : null,
      r.debut_ms !== null ? heure(r.debut_ms) + (r.fin_ms !== null ? `–${heure(r.fin_ms)}` : '') : null,
      dureeReunion(r),
    ]
      .filter(Boolean)
      .join(' · ');
  }

  function metaReunion(r: ResumeReunion): string {
    return [r.debut_ms !== null ? heure(r.debut_ms) : null, dureeReunion(r), r.a_compte_rendu ? t('carnet.main.metaCompteRendu') : null].filter(Boolean).join(' · ');
  }

  function entrees(index: Index): Entree[] {
    const out: Entree[] = [];
    if (etat.filtre !== 'conversation') {
      for (const r of index.reunions) {
        const ms = r.debut_ms ?? 0;
        out.push({
          genre: 'reunion',
          id: r.nom,
          jour: r.debut_ms !== null ? cleDuJour(r.debut_ms) : (r.date ?? '0000-00-00'),
          ms,
          titre: titreReunion(r),
          meta: metaReunion(r),
          apercu: r.apercu,
          enCours: r.en_cours,
        });
      }
    }
    if (etat.filtre !== 'reunion') {
      for (const c of index.conversations) {
        out.push({
          genre: 'conversation',
          id: c.id,
          jour: c.debut_ms !== null ? cleDuJour(c.debut_ms) : c.date,
          ms: c.debut_ms ?? 0,
          titre: titreConversation(c.apercu, c.titre),
          meta: [c.debut_ms !== null ? heure(c.debut_ms) : null, t('carnet.main.echanges', { n: c.tours })].filter(Boolean).join(' · '),
          apercu: '',
          enCours: false,
        });
      }
    }
    return out.sort((a, b) => b.jour.localeCompare(a.jour) || b.ms - a.ms);
  }

  function estSelectionne(genre: Genre, id: string): boolean {
    const s = etat.selection;
    return s !== null && 'genre' in s && s.genre === genre && s.id === id;
  }

  function ligne(e: Entree, i: number): HTMLElement {
    const bouton = ligneBouton(e, i);
    const titre = bouton.querySelector<HTMLElement>('.element-titre');
    return h(
      'div',
      { class: 'element-ligne', 'data-genre': e.genre, 'data-id': e.id },
      bouton,
      titre && crayon(e.titre, () => editerTitre(titre, e.titre, e.genre, e.id)),
    );
  }

  function ligneBouton(e: Entree, i: number): HTMLElement {
    return h(
      'button',
      {
        type: 'button',
        class: 'element',
        'data-genre': e.genre,
        'data-id': e.id,
        'aria-current': estSelectionne(e.genre, e.id) ? 'true' : null,
        style: `--i:${Math.min(i, 20)}`,
        onclick: () => void ouvrir(e.genre, e.id),
      },
      h('span', { class: 'genre', 'aria-hidden': 'true' }, e.genre === 'reunion' ? '●' : '»'),
      h(
        'span',
        { class: 'element-corps' },
        h('span', { class: 'element-titre' }, e.titre, e.enCours && h('span', { class: 'rec' }, 'REC')),
        h('span', { class: 'element-meta' }, e.genre === 'reunion' ? t('carnet.main.reunion') : t('carnet.main.conversation'), e.meta ? ` · ${e.meta}` : ''),
        e.apercu && h('span', { class: 'element-apercu' }, e.apercu),
      ),
    );
  }

  function dessinerListe(): void {
    const index = etat.index;
    if (etat.resultats !== null) {
      dessinerResultats(etat.resultats);
      return;
    }
    if (!index) {
      liste.replaceChildren();
      return;
    }
    const tout = entrees(index);
    if (tout.length === 0) {
      const rien = index.reunions.length + index.conversations.length === 0;
      liste.replaceChildren(
        h(
          'p',
          { class: 'vide' },
          rien ? t('carnet.main.listeRien') : t('carnet.main.listeRienCategorie'),
        ),
      );
      return;
    }
    const groupes: HTMLElement[] = [];
    let jour = '';
    let section: HTMLElement | null = null;
    tout.forEach((e, i) => {
      if (e.jour !== jour || !section) {
        jour = e.jour;
        section = h('section', { class: 'jour', 'data-jour': jour }, h('h2', { class: 'jour-titre' }, libelleDuJour(jour)));
        groupes.push(section);
      }
      section.append(ligne(e, i));
    });
    liste.replaceChildren(...groupes);
  }

  function dessinerRappels(): void {
    const avenir = (etat.index?.rappels ?? []).filter((r) => !r.fait);
    if (avenir.length === 0 || etat.resultats !== null) {
      rappels.replaceChildren();
      rappels.hidden = true;
      return;
    }
    rappels.hidden = false;
    rappels.replaceChildren(
      h('h2', { class: 'jour-titre' }, t('carnet.main.aVenir')),
      ...avenir.map((r) => {
        const e = echeance(r.echeance_ms);
        return h(
          'div',
          { class: 'rappel', 'data-id': r.id, 'data-retard': String(e.retard) },
          h('button', {
            type: 'button',
            class: 'coche',
            'aria-label': t('carnet.main.marquerFait', { texte: r.texte }),
            onclick: async (ev: Event) => {
              const ligne = (ev.currentTarget as HTMLElement).parentElement!;
              try {
                await carnet.rappelFait(r.id);
                ligne.classList.add('fait');
                notifier(t('carnet.main.rappelFait'));
                reagir('fait');
                window.setTimeout(() => void rafraichir(), animationsReduites() ? 0 : 450);
              } catch (err) {
                notifier(message(err), 'erreur');
              }
            },
          }),
          h('span', { class: 'rappel-corps' }, h('span', { class: 'rappel-texte' }, r.texte), h('span', { class: 'rappel-quand' }, e.retard ? t('carnet.main.enRetard', { quand: e.texte }) : e.texte)),
        );
      }),
    );
  }

  // ----- Recherche -----

  function dessinerResultats(resultats: Trouve[]): void {
    if (resultats.length === 0) {
      liste.replaceChildren(h('p', { class: 'vide' }, t('carnet.main.aucuneTrace', { requete: etat.requete.trim() })));
      return;
    }
    const index = etat.index;
    const groupes = new Map<string, { genre: Genre; id: string; titre: string; jour: string | null; trouves: Trouve[] }>();
    for (const tr of resultats) {
      const genre: Genre = tr.type;
      const id = tr.type === 'reunion' ? tr.nom : tr.id;
      const cle = `${genre}:${id}`;
      if (!groupes.has(cle)) {
        let titre = t('carnet.main.conversation');
        let jour: string | null = null;
        if (tr.type === 'reunion') {
          const r = index?.reunions.find((x) => x.nom === tr.nom);
          titre = r ? titreReunion(r) : (tr.titre ?? tr.nom);
          jour = r?.debut_ms != null ? cleDuJour(r.debut_ms) : (r?.date ?? null);
        } else {
          const c = index?.conversations.find((x) => x.id === tr.id);
          titre = titreConversation(c?.apercu ?? '', c?.titre);
          jour = c?.date ?? null;
        }
        groupes.set(cle, { genre, id, titre, jour, trouves: [] });
      }
      groupes.get(cle)!.trouves.push(tr);
    }
    const n = resultats.length;
    liste.replaceChildren(
      h('p', { class: 'compte' }, `${t('carnet.main.passages', { n })} · ${t('carnet.main.elements', { n: groupes.size })}`),
      ...[...groupes.values()].map((g, i) =>
        h(
          'section',
          { class: 'resultat', 'data-genre': g.genre, 'data-id': g.id, style: `--i:${Math.min(i, 20)}` },
          h(
            'h2',
            { class: 'resultat-titre' },
            h('span', { class: 'genre', 'aria-hidden': 'true' }, g.genre === 'reunion' ? '●' : '»'),
            g.titre,
            g.jour && h('span', { class: 'resultat-jour' }, libelleDuJour(g.jour)),
          ),
          ...g.trouves.slice(0, 4).map((tr) =>
            h(
              'button',
              {
                type: 'button',
                class: 'extrait',
                onclick: () =>
                  void ouvrir(
                    g.genre,
                    g.id,
                    tr.type === 'reunion' ? { onglet: tr.partie, start: tr.start } : { at_ms: tr.at_ms },
                  ),
              },
              h(
                'span',
                { class: 'extrait-ou' },
                tr.type === 'reunion' ? (tr.partie === 'transcription' ? horodatage(tr.start ?? 0) : tr.partie === 'notes' ? t('carnet.main.extraitNotes') : t('carnet.main.extraitCompteRendu')) : heure(tr.at_ms),
              ),
              h('span', { class: 'extrait-texte' }, ...surligner(sansMarque(tr.extrait), etat.requete)),
            ),
          ),
          g.trouves.length > 4 && h('p', { class: 'encore' }, t('carnet.main.autres', { n: g.trouves.length - 4 })),
        ),
      ),
    );
  }

  /** Extrait d'une ligne de markdown sans sa puce, sa case, son dièse de titre ni ses marques en ligne. */
  function sansMarque(ligne: string): string {
    return ligne
      .replace(/^\s*(?:#{1,6}\s+|(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s*)?)/, '')
      .replace(/\*\*([^*]+)\*\*|`([^`]+)`/g, '$1$2');
  }

  let minuterieRecherche = 0;
  let numeroRecherche = 0;
  async function chercher(): Promise<void> {
    const requete = recherche.value;
    etat.requete = requete;
    const n = ++numeroRecherche;
    if (!requete.trim()) {
      etat.resultats = null;
      app.dataset.recherche = 'false';
      dessinerRappels();
      dessinerListe();
      return;
    }
    try {
      const resultats = await carnet.chercher(requete);
      if (n !== numeroRecherche) return;
      // Rafraîchissement sans changement (retour sur la fenêtre) : la liste
      // reste en place, un clic en cours ne tombe pas dans le vide.
      if (etat.resultats !== null && JSON.stringify(resultats) === JSON.stringify(etat.resultats)) return;
      etat.resultats = resultats;
      app.dataset.recherche = 'true';
      dessinerRappels();
      dessinerListe();
    } catch (e) {
      notifier(message(e), 'erreur');
    }
  }
  recherche.addEventListener('input', () => {
    // Le petit Kobi de l'app réfléchit avec toi (pas à un simple rafraîchissement).
    if (recherche.value.trim()) reagir('cherche');
    window.clearTimeout(minuterieRecherche);
    minuterieRecherche = window.setTimeout(() => void chercher(), 160);
  });
  recherche.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      window.clearTimeout(minuterieRecherche);
      void chercher();
    } else if (e.key === 'Escape' && recherche.value) {
      e.stopPropagation();
      recherche.value = '';
      void chercher();
    }
  });
  window.addEventListener('keydown', (e) => {
    if (hote.closest('[hidden]')) return;
    if ((e.key === 'f' && (e.ctrlKey || e.metaKey)) || (e.key === '/' && document.activeElement?.tagName !== 'INPUT')) {
      e.preventDefault();
      recherche.focus();
      recherche.select();
    }
  });

  // ----- Page -----

  let numeroOuverture = 0;

  function marquerSelection(): void {
    for (const b of liste.querySelectorAll<HTMLElement>('.element')) {
      const actif = estSelectionne(b.dataset.genre as Genre, b.dataset.id!);
      if (actif) b.setAttribute('aria-current', 'true');
      else b.removeAttribute('aria-current');
    }
    app.dataset.page = etat.selection === null ? 'vide' : 'genre' in etat.selection ? etat.selection.genre : 'stockage';
  }

  /** La page vide montrée dit-elle « rien encore » ? `null` : pas de page vide. */
  let pageVideRien: boolean | null = null;

  function pageVide(): void {
    etat.selection = null;
    marquerSelection();
    const vide = (etat.index?.reunions.length ?? 0) + (etat.index?.conversations.length ?? 0) === 0;
    const grand = h('h2', { class: 'vide-titre' });
    page.replaceChildren(
      h(
        'div',
        { class: 'feuille vide-page' },
        grand,
        h('p', { class: 'vide-aide' }, vide ? t('carnet.main.videAideRien') : t('carnet.main.videAide')),
      ),
    );
    poserTitre(grand, vide ? t('carnet.main.videTitreRien') : t('carnet.main.videTitre'));
    pageVideRien = vide;
  }

  async function ouvrir(genre: Genre, id: string, cible: Cible = {}): Promise<void> {
    const n = ++numeroOuverture;
    pageVideRien = null;
    const meme = estSelectionne(genre, id);
    etat.selection = { genre, id };
    if (cible.onglet) etat.onglet = cible.onglet;
    else if (!meme) etat.onglet = 'compte-rendu';
    marquerSelection();
    try {
      if (genre === 'reunion') {
        const r = await carnet.reunion(id);
        if (n !== numeroOuverture) return;
        if (!cible.onglet && !meme && !r.compte_rendu) etat.onglet = r.segments.length ? 'transcription' : 'notes';
        pageReunion(r, cible, !meme || cible.onglet !== undefined);
      } else {
        const c = await carnet.conversation(id);
        if (n !== numeroOuverture) return;
        pageConversation(c, cible, !meme || cible.at_ms !== undefined);
      }
    } catch (e) {
      if (n !== numeroOuverture) return;
      notifier(message(e), 'erreur');
      pageVide();
    }
  }

  /** Retour d'une copie : ce qui est copié, sa taille, son format, ses premières lignes. */
  const quandCopie: QuandCopie = (r) => (r instanceof Error ? notifier(message(r), 'erreur') : notifier(r.message, 'ok', { apercu: r.apercu }));

  /** Ce que contient le markdown d'export (stockage.rs, document_reunion). */
  function contenuExport(genre: Genre, r?: ReunionLue): string {
    if (genre === 'conversation' || !r) return t('carnet.main.exportConversation');
    const corps = r.compte_rendu ? t('carnet.main.exportCompteRendu') : r.notes.trim() ? t('carnet.main.exportNotes') : null;
    return corps ? t('carnet.main.exportReunionAvec', { corps }) : t('carnet.main.exportReunion');
  }

  /**
   * « Publier » (KOB-75) : le compte rendu part vers la destination réglée
   * (Réglages › Stockage), par Kobi et le MCP, après sa carte de
   * confirmation. Sans destination, le clic propose de la régler. La
   * destination est relue à chaque clic (KOB-264) : réglée après coup, elle
   * vaut sans rouvrir la réunion.
   */
  function boutonPublier(nom: string): HTMLElement {
    let destination: { mcp: string; destination: string } | null = null;
    const relire = async (): Promise<void> => {
      try {
        destination = await carnet.publication();
      } catch {
        return;
      }
      bouton.dataset.reglee = String(Boolean(destination));
      bouton.title = destination
        ? t('carnet.main.publierAide', { mcp: destination.mcp, destination: destination.destination || destination.mcp })
        : t('carnet.main.publierNonRegle');
    };
    const bouton = h(
      'button',
      {
        type: 'button',
        class: 'bouton',
        'data-action': 'publier',
        title: t('carnet.main.publierNonRegle'),
        onpointerenter: () => void relire(),
        onfocus: () => void relire(),
        onclick: async () => {
          await relire();
          if (!destination) {
            notifier(t('carnet.main.publierNonRegle'), 'ok', { action: { libelle: t('carnet.main.regler'), faire: () => ouvrirStockage() } });
            return;
          }
          try {
            notifier(await carnet.publier(nom), 'ok', { apercu: t('carnet.main.publierApercu') });
          } catch (e) {
            notifier(message(e), 'erreur');
          }
        },
      },
      t('carnet.main.publier'),
    );
    void relire();
    return bouton;
  }

  function actions(genre: Genre, id: string, nom: string, r?: ReunionLue): HTMLElement {
    const barre = h('div', { class: 'actions' });
    const copie =
      genre === 'reunion' && r
        ? menuCopier(
            partiesReunion(r, () => carnet.markdown(genre, id)),
            quandCopie,
          )
        : h(
            'button',
            {
              type: 'button',
              class: 'bouton',
              'data-action': 'copier',
              onclick: async () => {
                const quoi: Quoi = 'conversation';
                try {
                  quandCopie(await copier(await carnet.markdown(genre, id), quoi, formatCopie()));
                } catch (e) {
                  notifier(message(e), 'erreur');
                }
              },
            },
            t('carnet.main.copierConversation'),
          );
    const boutons = h(
      'div',
      { class: 'actions-boutons' },
      h(
        'button',
        {
          type: 'button',
          class: 'bouton',
          'data-action': 'dossier',
          onclick: () => carnet.ouvrirDossier(genre, id).catch((e) => notifier(message(e), 'erreur')),
        },
        t('carnet.main.ouvrirDossier'),
      ),
      copie,
      h(
        'button',
        {
          type: 'button',
          class: 'bouton',
          'data-action': 'exporter',
          title: t('carnet.main.exporterAide', { contenu: contenuExport(genre, r) }),
          onclick: async () => {
            try {
              const chemin = await carnet.exporter(genre, id);
              if (!chemin) return;
              const fichier = chemin.split(/[\\/]/).pop() ?? chemin;
              const dossier = chemin.slice(0, chemin.length - fichier.length);
              notifier(t('carnet.main.exporte', { fichier }), 'ok', {
                apercu: t('carnet.main.exporteApercu', { contenu: contenuExport(genre, r), dossier }),
                action: { libelle: t('carnet.main.ouvrir'), faire: () => void carnet.ouvrirExport().catch((e) => notifier(message(e), 'erreur')) },
              });
            } catch (e) {
              notifier(message(e), 'erreur');
            }
          },
        },
        t('carnet.main.exporter'),
      ),
      genre === 'reunion' ? boutonPublier(id) : null,
      h('button', { type: 'button', class: 'bouton danger', 'data-action': 'supprimer', onclick: () => confirmer() }, t('carnet.main.supprimer')),
    );
    const confirmer = () => {
      const annuler = h('button', { type: 'button', class: 'bouton', 'data-action': 'annuler', onclick: () => fermer() }, t('carnet.main.annuler'));
      const confirmation = h(
        'div',
        { class: 'confirmation', role: 'alertdialog', 'aria-label': t('carnet.main.confirmerSuppression') },
        h(
          'p',
          {},
          h('strong', {}, t('carnet.main.supprimerQuestion', { nom })),
          ' ',
          genre === 'reunion' ? t('carnet.main.supprimerReunionAide') : t('carnet.main.supprimerConversationAide'),
        ),
        h(
          'div',
          { class: 'actions-boutons' },
          annuler,
          h(
            'button',
            {
              type: 'button',
              class: 'bouton plein danger',
              'data-action': 'confirmer',
              onclick: async () => {
                try {
                  await carnet.supprimer(genre, id);
                  notifier(genre === 'reunion' ? t('carnet.main.reunionSupprimee') : t('carnet.main.conversationSupprimee'));
                  reagir('supprime');
                  pageVide();
                  await rafraichir();
                } catch (e) {
                  notifier(message(e), 'erreur');
                  fermer();
                }
              },
            },
            t('carnet.main.supprimerDefinitivement'),
          ),
        ),
      );
      const fermer = () => {
        confirmation.remove();
        boutons.hidden = false;
        window.removeEventListener('keydown', echap, true);
        (boutons.querySelector('[data-action="supprimer"]') as HTMLElement | null)?.focus();
      };
      const echap = (e: KeyboardEvent) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          fermer();
        }
      };
      window.addEventListener('keydown', echap, true);
      boutons.hidden = true;
      barre.append(confirmation);
      annuler.focus();
    };
    barre.append(boutons);
    return barre;
  }

  function entete(surtitre: string, titre: string, genre: Genre, id: string, animer: boolean, enCours = false, r?: ReunionLue): HTMLElement {
    const h1 = h('h1', { class: 'titre', title: t('carnet.main.renommer') });
    if (animer) poserTitre(h1, titre);
    else {
      h1.textContent = titre;
      h1.setAttribute('aria-label', titre);
    }
    // Titre éditable sur place (KOB-223) : clic sur le titre ou crayon.
    h1.addEventListener('click', () => editerTitre(h1, titre, genre, id));
    return h(
      'header',
      { class: 'entete' },
      h('p', { class: 'surtitre' }, enCours && h('span', { class: 'rec' }, 'REC'), surtitre),
      h('div', { class: 'titre-ligne' }, h1, crayon(titre, () => editerTitre(h1, titre, genre, id))),
      actions(genre, id, titre, r),
    );
  }

  /** Petit bouton « Renommer » (crayon dessiné, pas d'emoji). */
  function crayon(titre: string, editer: () => void): HTMLElement {
    const b = h('button', { type: 'button', class: 'crayon', 'aria-label': t('carnet.main.renommerAria', { titre }), title: t('carnet.main.renommer') });
    b.innerHTML =
      '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M3 13l1-3.5 7-7 2.5 2.5-7 7zM9.5 4l2.5 2.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/></svg>';
    b.addEventListener('click', (ev) => {
      ev.stopPropagation();
      editer();
    });
    return b;
  }

  /**
   * Remplace le titre par un champ prérempli et sélectionné : Entrée
   * valide, Échap ou clic ailleurs annule. Le nouveau titre s'affiche tout
   * de suite ; l'ancien revient si Kobi refuse.
   */
  function editerTitre(cible: HTMLElement, actuel: string, genre: Genre, id: string): void {
    if (cible.querySelector('input')) return;
    const avant = Array.from(cible.childNodes);
    const champ = h('input', {
      type: 'text',
      class: 'titre-champ',
      value: actuel,
      maxlength: '120',
      'aria-label': genre === 'reunion' ? t('carnet.main.titreReunion') : t('carnet.main.titreConversation'),
      spellcheck: 'true',
    }) as HTMLInputElement;
    cible.replaceChildren(champ);
    champ.focus();
    champ.select();
    let fini = false;
    const remettre = (texte?: string): void => {
      if (texte === undefined) cible.replaceChildren(...avant);
      else {
        cible.textContent = texte;
        cible.setAttribute('aria-label', texte);
      }
    };
    const valider = async (): Promise<void> => {
      const titre = champ.value.trim();
      if (fini) return;
      fini = true;
      if (!titre || titre === actuel) {
        remettre();
        return;
      }
      remettre(titre);
      const ouverte = estSelectionne(genre, id);
      try {
        const nouveau = await carnet.renommer(genre, id, titre);
        notifier(t('carnet.main.titreEnregistre'));
        // Le dossier a pu changer de nom : la vue ouverte le suit.
        if (ouverte) etat.selection = { genre, id: nouveau };
        await rafraichir();
        if (ouverte) await ouvrir(genre, nouveau);
      } catch (e) {
        remettre();
        notifier(message(e), 'erreur');
      }
    };
    champ.addEventListener('keydown', (ev) => {
      ev.stopPropagation();
      if (ev.key === 'Enter') {
        ev.preventDefault();
        void valider();
      } else if (ev.key === 'Escape') {
        ev.preventDefault();
        fini = true;
        remettre();
      }
    });
    champ.addEventListener('click', (ev) => ev.stopPropagation());
    champ.addEventListener('blur', () => {
      if (!fini) {
        fini = true;
        remettre();
      }
    });
  }

  function section(titre: string, cle: string, texte: string, mot: string): HTMLElement | null {
    if (!texte.trim()) return null;
    const bloc = h('section', { class: 'bloc copiable', 'data-section': cle }, h('h3', { class: 'bloc-titre' }, titre), markdown(texte, mot));
    // Copier toute la partie, ou une seule décision, action, question (KOB-224).
    bloc.append(boutonCopie(PARTIE[cle] ?? 'section', () => texte.trim(), quandCopie));
    const element = ELEMENT[cle];
    if (element) copierPuces(bloc, texte, element, quandCopie);
    return bloc;
  }

  function pageReunion(r: ReunionLue, cible: Cible, animer: boolean): void {
    const mot = etat.resultats !== null ? etat.requete : '';
    const titre = titreReunion(r.resume);
    const surtitre = surtitreReunion(r.resume);

    const contenu = h('div', { class: 'contenu' });
    const onglets = h('div', { class: 'onglets', role: 'tablist', 'aria-label': t('carnet.main.partiesReunion') });
    const indicateur = h('span', { class: 'onglet-indicateur', 'aria-hidden': 'true' });
    const liste: Array<[Onglet, string, number | null]> = [
      ['compte-rendu', t('carnet.main.ongletCompteRendu'), null],
      ['transcription', t('carnet.main.ongletTranscription'), r.segments.length],
      ['notes', t('carnet.main.ongletNotes'), null],
    ];
    // Le soulignement suit l'onglet actif, mesuré à chaque changement de
    // taille : la graisse change en transition après le clic (KOB-246, 4.4).
    const souligner = () => {
      const b = onglets.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]');
      if (!b) return;
      indicateur.style.setProperty('--x', `${b.offsetLeft}px`);
      indicateur.style.setProperty('--l', `${b.offsetWidth}px`);
    };
    const mesure = new ResizeObserver(souligner);
    const montrer = (o: Onglet, defiler?: Cible) => {
      etat.onglet = o;
      for (const b of onglets.querySelectorAll<HTMLElement>('[role="tab"]')) {
        const actif = b.dataset.onglet === o;
        b.setAttribute('aria-selected', String(actif));
        b.tabIndex = actif ? 0 : -1;
      }
      souligner();
      contenu.dataset.onglet = o;
      contenu.replaceChildren(o === 'compte-rendu' ? compteRendu(r, mot) : o === 'transcription' ? transcription(r, mot) : notes(r, mot));
      if (defiler) allerA(contenu, defiler);
    };
    for (const [o, libelle, n] of liste) {
      const onglet = h(
        'button',
        { type: 'button', role: 'tab', class: 'onglet', 'data-onglet': o, onclick: () => montrer(o) },
        h('span', { class: 'libelle', 'data-libelle': libelle }, libelle),
        n !== null && n > 0 && h('span', { class: 'nombre' }, String(n)),
      );
      mesure.observe(onglet);
      onglets.append(onglet);
    }
    onglets.append(indicateur);
    onglets.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      const i = liste.findIndex(([o]) => o === etat.onglet);
      const suivant = liste[(i + (e.key === 'ArrowRight' ? 1 : liste.length - 1)) % liste.length]![0];
      montrer(suivant);
      onglets.querySelector<HTMLElement>(`[data-onglet="${suivant}"]`)?.focus();
    });

    const feuille = h('article', { class: 'feuille', 'data-genre': 'reunion', 'data-id': r.resume.nom }, entete(surtitre, titre, 'reunion', r.resume.nom, animer, r.resume.en_cours, r), onglets, contenu);
    if (!animer) feuille.classList.add('sans-entree');
    page.replaceChildren(feuille);
    montrer(etat.onglet, cible);
  }

  function compteRendu(r: ReunionLue, mot: string): HTMLElement {
    const cr = r.compte_rendu;
    if (!cr) {
      return h(
        'div',
        { class: 'vide-onglet' },
        h('p', {}, r.resume.en_cours ? t('carnet.main.compteRenduEnCours') : t('carnet.main.compteRenduAucun')),
        r.segments.length > 0 && h('p', {}, t('carnet.main.transcriptionVoisine')),
      );
    }
    const blocs = [
      section(t('carnet.main.resume'), 'resume', cr.resume, mot),
      section(t('carnet.main.decisions'), 'decisions', cr.decisions, mot),
      section(t('carnet.main.actionsAFaire'), 'actions', cr.actions, mot),
      section(t('carnet.main.questionsOuvertes'), 'questions', cr.questions, mot),
      ...cr.autres.map(([titre, texte]) => section(titre, 'autre', texte, mot)),
    ].filter((b): b is HTMLElement => b !== null);
    if (blocs.length === 0) blocs.push(h('section', { class: 'bloc' }, markdown(cr.brut, mot)));
    blocs.forEach((b, i) => b.style.setProperty('--i', String(i)));
    return h('div', { class: 'compte-rendu' }, ...blocs);
  }

  function transcription(r: ReunionLue, mot: string): HTMLElement {
    if (r.segments.length === 0) return h('div', { class: 'vide-onglet' }, h('p', {}, t('carnet.main.aucuneParole')));
    return h(
      'ol',
      { class: 'transcription' },
      ...r.segments.map((s, i) =>
        h(
          'li',
          { class: 'parole', 'data-source': s.source, 'data-start': String(s.start), style: `--i:${Math.min(i, 30)}` },
          h('span', { class: 'temps' }, horodatage(s.start)),
          h('span', { class: 'qui' }, s.source === 'moi' ? t('carnet.main.moi') : t('carnet.main.eux')),
          h('span', { class: 'dit' }, ...surligner(s.text, mot)),
          boutonCopie('ligne', () => ligneMd(s), quandCopie),
        ),
      ),
    );
  }

  function notes(r: ReunionLue, mot: string): HTMLElement {
    if (!r.notes.trim()) return h('div', { class: 'vide-onglet' }, h('p', {}, t('carnet.main.notesAucune')));
    const bloc = h('div', { class: 'notes bloc' }, markdown(r.notes, mot));
    copierPuces(bloc, r.notes, 'note', quandCopie);
    return bloc;
  }

  function pageConversation(c: ConversationLue, cible: Cible, animer: boolean): void {
    const mot = etat.resultats !== null ? etat.requete : '';
    const jour = c.resume.debut_ms !== null ? cleDuJour(c.resume.debut_ms) : c.resume.date;
    const surtitre = [t('carnet.main.conversation'), libelleDuJour(jour), c.resume.debut_ms !== null ? heure(c.resume.debut_ms) : null, t('carnet.main.echanges', { n: c.resume.tours })]
      .filter(Boolean)
      .join(' · ');
    const contenu = h(
      'ol',
      { class: 'tours' },
      ...c.tours.map((tour, i) =>
        h(
          'li',
          { class: 'tour', 'data-qui': tour.qui, 'data-at': String(tour.at_ms), style: `--i:${Math.min(i, 30)}` },
          h('span', { class: 'tour-tete' }, tour.qui === 'moi' ? t('carnet.main.moi') : 'Kobi', ` · ${heure(tour.at_ms)}`, tour.voix && h('span', { class: 'voix', title: t('carnet.main.alaVoixTitre') }, ` · ${t('carnet.main.alaVoix')}`)),
          tour.qui === 'kobi' ? h('div', { class: 'tour-texte' }, markdown(tour.texte, mot)) : h('p', { class: 'tour-texte' }, ...surligner(tour.texte, mot)),
        ),
      ),
    );
    const feuille = h(
      'article',
      { class: 'feuille', 'data-genre': 'conversation', 'data-id': c.resume.id },
      entete(surtitre, titreConversation(c.resume.apercu, c.resume.titre), 'conversation', c.resume.id, animer),
      h('div', { class: 'contenu' }, contenu),
    );
    if (!animer) feuille.classList.add('sans-entree');
    page.replaceChildren(feuille);
    allerA(contenu, cible);
  }

  /** Fait défiler jusqu'au passage trouvé et le fait clignoter. */
  function allerA(contenu: HTMLElement, cible: Cible): void {
    let el: HTMLElement | null = null;
    if (cible.start != null) el = contenu.querySelector<HTMLElement>(`[data-start="${cible.start}"]`);
    else if (cible.at_ms !== undefined) el = contenu.querySelector<HTMLElement>(`[data-at="${cible.at_ms}"]`);
    else if (etat.resultats !== null && etat.requete.trim()) el = contenu.querySelector('mark')?.closest<HTMLElement>('li, p, section') ?? null;
    if (!el) return;
    el.classList.add('cible');
    requestAnimationFrame(() => el!.scrollIntoView({ block: 'center', behavior: animationsReduites() ? 'auto' : 'smooth' }));
  }

  // ----- Stockage -----

  function ouvrirStockage(): void {
    if (options.reglages) return options.reglages('stockage');
    numeroOuverture++;
    pageVideRien = null;
    etat.selection = { panneau: 'stockage' };
    marquerSelection();
    const corps = h('div', { class: 'panneau', 'data-panneau': STOCKAGE.id });
    const titre = h('h1', { class: 'titre' });
    page.replaceChildren(
      h(
        'article',
        { class: 'feuille' },
        h('header', { class: 'entete' }, h('p', { class: 'surtitre' }, `${t('carnet.main.reglages')} · `, STOCKAGE.sousTitre), titre),
        corps,
      ),
    );
    poserTitre(titre, STOCKAGE.titre);
    STOCKAGE.monter(corps, {
      notifier: (texte, ton) => {
        notifier(texte, ton);
        if (ton !== 'erreur') void rafraichir();
      },
    });
  }

  // ----- Données -----

  async function rafraichir(): Promise<void> {
    try {
      etat.index = await carnet.index();
    } catch (e) {
      notifier(message(e), 'erreur');
      return;
    }
    racine.textContent = etat.index.racine;
    racine.title = etat.index.racine;
    const s = etat.selection;
    if (s && 'genre' in s) {
      const existe = s.genre === 'reunion' ? etat.index.reunions.some((r) => r.nom === s.id) : etat.index.conversations.some((c) => c.id === s.id);
      if (!existe) pageVide();
    } else if (s === null && pageVideRien !== null && pageVideRien !== (etat.index.reunions.length + etat.index.conversations.length === 0)) {
      pageVide();
    }
    dessinerRappels();
    if (etat.resultats !== null) await chercher();
    else dessinerListe();
    marquerSelection();
  }

  // Rafraîchi à chaque retour sur la fenêtre : une réunion ou une
  // conversation a pu s'ajouter pendant qu'elle était en arrière-plan.
  window.addEventListener('focus', () => {
    if (!hote.closest('[hidden]')) void rafraichir();
  });

  // La réunion en cours a reçu un titre (KOB-223) : son dossier a peut-être
  // changé de nom ; la vue ouverte suit.
  ecouter<{ ancien: string | null; reunion: string; titre: string | null }>('reunion-nommee', (n) => {
    const s = etat.selection;
    const ouverte = s && 'genre' in s && s.genre === 'reunion' && (s.id === n.ancien || s.id === n.reunion);
    if (ouverte) etat.selection = { genre: 'reunion', id: n.reunion };
    void rafraichir().then(() => (ouverte ? ouvrir('reunion', n.reunion) : undefined));
  });

  // Durée d'une réunion en cours (KOB-251) : une fois par minute, seul le
  // texte de sa ligne et de son en-tête change, sans redessiner la liste.
  window.setInterval(() => {
    for (const r of etat.index?.reunions ?? []) {
      if (!r.en_cours) continue;
      const nom = CSS.escape(r.nom);
      const meta = hote.querySelector<HTMLElement>(`.element[data-genre="reunion"][data-id="${nom}"] .element-meta`);
      if (meta) meta.textContent = `${t('carnet.main.reunion')} · ${metaReunion(r)}`;
      const sur = hote.querySelector<HTMLElement>(`.feuille[data-genre="reunion"][data-id="${nom}"] .surtitre`)?.lastChild;
      if (sur?.nodeType === Node.TEXT_NODE) sur.textContent = surtitreReunion(r);
    }
  }, 60_000);

  // Changement de langue (KOB-208) : squelette, rappels, liste et vue
  // ouverte redessinés, la sélection (et l'onglet) gardée.
  surLangue(() => {
    textesSquelette();
    dessinerRappels();
    dessinerListe();
    marquerSelection();
    const s = etat.selection;
    if (s === null) pageVide();
    else if ('panneau' in s) ouvrirStockage();
    else void ouvrir(s.genre, s.id);
  });

  window.carnet = { rafraichir, ouvrir, normaliser };

  pageVide();
  if (disponible()) void rafraichir();
  else notifier(t('carnet.main.horsKobi'), 'erreur');
  return {
    rafraichir,
    ouvrir,
    ouvrirReunion: async (nom: string) => {
      await rafraichir();
      await ouvrir('reunion', nom, { onglet: 'compte-rendu' });
    },
  };
}

/** Accès pour les tests. */
declare global {
  interface Window {
    carnet: { rafraichir(): Promise<void>; ouvrir(genre: Genre, id: string): Promise<void>; normaliser(t: string): string };
  }
}
