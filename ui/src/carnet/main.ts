// Le Carnet (KOB-140), section « Notes et réunions » de l'app Kobi
// (KOB-159) : relire réunions, conversations et rappels rangés par Kobi.
// Index à gauche (recherche, filtres, rappels, liste par jour), page à
// droite (compte rendu, transcription, notes, ou conversation). Toutes les
// données viennent des commandes `carnet_*` de kobi-app.

import './carnet.css';
import { animationsReduites, h, normaliser, surligner } from '../dom';
import { disponible, message } from '../ipc';
import { Blinker } from '../mascotte/glyphs';
import { creerNotifieur } from '../notif';
import { STOCKAGE } from '../reglages/stockage';
import { poserTitre } from '../titre';
import { carnet, type ConversationLue, type Genre, type Index, type ReunionLue, type Trouve } from './donnees';
import { cleDuJour, duree, echeance, heure, horodatage, libelleDuJour, markdown } from './format';

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

  const yeux = [h('span', { class: 'oeil' }, '*'), h('span', { class: 'oeil' }, '*')];
  const recherche = h('input', {
    type: 'search',
    class: 'champ',
    placeholder: 'Chercher un mot…',
    'aria-label': 'Chercher dans les réunions et conversations',
    spellcheck: 'false',
    autocomplete: 'off',
  });
  const filtres = h('div', { class: 'filtres', role: 'group', 'aria-label': 'Afficher' });
  const rappels = h('section', { class: 'rappels', 'aria-label': 'Rappels à venir' });
  const liste = h('nav', { class: 'liste', 'aria-label': 'Réunions et conversations' });
  const racine = h('code', { class: 'racine' });
  const page = h('main', { class: 'page', 'aria-live': 'polite' });
  const app = h(
    'div',
    { class: 'carnet' },
    h(
      'aside',
      { class: 'index' },
      h('header', { class: 'marque' }, h('span', { class: 'mini-kobi', 'aria-hidden': 'true' }, ...yeux), h('h1', { class: 'marque-mot' }, 'carnet')),
      h('div', { class: 'recherche' }, h('span', { class: 'loupe', 'aria-hidden': 'true' }, '?'), recherche),
      filtres,
      h('div', { class: 'defile' }, rappels, liste),
      h(
        'footer',
        { class: 'pied' },
        h('button', { type: 'button', class: 'bouton-pied', 'data-action': 'stockage', onclick: () => ouvrirStockage() }, h('span', { 'aria-hidden': 'true' }, '⚙'), ' Stockage'),
        racine,
      ),
    ),
    page,
  );
  hote.append(app);
  const notifier = creerNotifieur(document.body);

  const blinker = new Blinker((ferme) => yeux.forEach((o) => (o.textContent = ferme ? '–' : '*')));
  blinker.run(!animationsReduites() && !document.hidden);
  document.addEventListener('visibilitychange', () => blinker.run(!animationsReduites() && !document.hidden));

  for (const [valeur, libelle] of [
    ['tout', 'Tout'],
    ['reunion', 'Réunions'],
    ['conversation', 'Conversations'],
  ] as const) {
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
        libelle,
      ),
    );
  }

  // ----- Index -----

  function titreReunion(r: { titre: string | null; debut_ms: number | null; date: string | null }): string {
    if (r.titre) return r.titre;
    const cle = r.debut_ms !== null ? cleDuJour(r.debut_ms) : r.date;
    return cle ? `Réunion ${libelleDuJour(cle).toLowerCase()}` : 'Réunion';
  }

  function titreConversation(apercu: string): string {
    const t = apercu.trim();
    return t ? (t.length > 70 ? `${t.slice(0, 68)}…` : t) : 'Conversation';
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
          meta: [r.debut_ms !== null ? heure(r.debut_ms) : null, r.duree_s > 0 ? duree(r.duree_s) : null, r.a_compte_rendu ? 'compte rendu' : null]
            .filter(Boolean)
            .join(' · '),
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
          titre: titreConversation(c.apercu),
          meta: [c.debut_ms !== null ? heure(c.debut_ms) : null, `${c.tours} ${c.tours > 1 ? 'échanges' : 'échange'}`].filter(Boolean).join(' · '),
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
        h('span', { class: 'element-meta' }, e.genre === 'reunion' ? 'Réunion' : 'Conversation', e.meta ? ` · ${e.meta}` : ''),
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
          rien ? 'Rien encore. Dis « Kobi, enregistre la réunion » ou parle-lui : tout arrivera ici.' : 'Rien dans cette catégorie.',
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
      h('h2', { class: 'jour-titre' }, 'À venir'),
      ...avenir.map((r) => {
        const e = echeance(r.echeance_ms);
        return h(
          'div',
          { class: 'rappel', 'data-id': r.id, 'data-retard': String(e.retard) },
          h('button', {
            type: 'button',
            class: 'coche',
            'aria-label': `Marquer « ${r.texte} » comme fait`,
            onclick: async (ev: Event) => {
              const ligne = (ev.currentTarget as HTMLElement).parentElement!;
              try {
                await carnet.rappelFait(r.id);
                ligne.classList.add('fait');
                notifier('Rappel fait.');
                window.setTimeout(() => void rafraichir(), animationsReduites() ? 0 : 450);
              } catch (err) {
                notifier(message(err), 'erreur');
              }
            },
          }),
          h('span', { class: 'rappel-corps' }, h('span', { class: 'rappel-texte' }, r.texte), h('span', { class: 'rappel-quand' }, e.retard ? `En retard · ${e.texte}` : e.texte)),
        );
      }),
    );
  }

  // ----- Recherche -----

  function dessinerResultats(resultats: Trouve[]): void {
    if (resultats.length === 0) {
      liste.replaceChildren(h('p', { class: 'vide' }, `Aucune trace de « ${etat.requete.trim()} ».`));
      return;
    }
    const index = etat.index;
    const groupes = new Map<string, { genre: Genre; id: string; titre: string; jour: string | null; trouves: Trouve[] }>();
    for (const t of resultats) {
      const genre: Genre = t.type;
      const id = t.type === 'reunion' ? t.nom : t.id;
      const cle = `${genre}:${id}`;
      if (!groupes.has(cle)) {
        let titre = 'Conversation';
        let jour: string | null = null;
        if (t.type === 'reunion') {
          const r = index?.reunions.find((x) => x.nom === t.nom);
          titre = r ? titreReunion(r) : (t.titre ?? t.nom);
          jour = r?.debut_ms != null ? cleDuJour(r.debut_ms) : (r?.date ?? null);
        } else {
          const c = index?.conversations.find((x) => x.id === t.id);
          titre = titreConversation(c?.apercu ?? '');
          jour = c?.date ?? null;
        }
        groupes.set(cle, { genre, id, titre, jour, trouves: [] });
      }
      groupes.get(cle)!.trouves.push(t);
    }
    const n = resultats.length;
    liste.replaceChildren(
      h('p', { class: 'compte' }, `${n} ${n > 1 ? 'passages' : 'passage'} · ${groupes.size} ${groupes.size > 1 ? 'éléments' : 'élément'}`),
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
          ...g.trouves.slice(0, 4).map((t) =>
            h(
              'button',
              {
                type: 'button',
                class: 'extrait',
                onclick: () =>
                  void ouvrir(
                    g.genre,
                    g.id,
                    t.type === 'reunion' ? { onglet: t.partie, start: t.start } : { at_ms: t.at_ms },
                  ),
              },
              h(
                'span',
                { class: 'extrait-ou' },
                t.type === 'reunion' ? (t.partie === 'transcription' ? horodatage(t.start ?? 0) : t.partie === 'notes' ? 'notes' : 'compte rendu') : heure(t.at_ms),
              ),
              h('span', { class: 'extrait-texte' }, ...surligner(sansMarque(t.extrait), etat.requete)),
            ),
          ),
          g.trouves.length > 4 && h('p', { class: 'encore' }, `+ ${g.trouves.length - 4} autres`),
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
  /** Clignements du grand Kobi de la page vide. */
  let grandsYeux: Blinker | null = null;

  function pageVide(): void {
    etat.selection = null;
    marquerSelection();
    const vide = (etat.index?.reunions.length ?? 0) + (etat.index?.conversations.length ?? 0) === 0;
    const grand = h('h2', { class: 'vide-titre' });
    const yeuxVides = [h('span', {}, '^'), h('span', {}, '^')];
    grandsYeux?.run(false);
    grandsYeux = new Blinker((ferme) => yeuxVides.forEach((o) => (o.textContent = ferme ? '–' : '^')));
    grandsYeux.run(!animationsReduites());
    page.replaceChildren(
      h(
        'div',
        { class: 'feuille vide-page' },
        h('div', { class: 'grand-kobi', 'aria-hidden': 'true' }, ...yeuxVides),
        grand,
        h('p', { class: 'vide-aide' }, vide ? 'Les réunions enregistrées et les conversations avec Kobi apparaîtront ici.' : 'Choisis une réunion ou une conversation à gauche, ou cherche un mot.'),
      ),
    );
    poserTitre(grand, vide ? 'rien encore' : 'à relire');
    pageVideRien = vide;
  }

  async function ouvrir(genre: Genre, id: string, cible: Cible = {}): Promise<void> {
    const n = ++numeroOuverture;
    pageVideRien = null;
    grandsYeux?.run(false);
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

  function actions(genre: Genre, id: string, nom: string): HTMLElement {
    const barre = h('div', { class: 'actions' });
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
        'Ouvrir le dossier',
      ),
      h(
        'button',
        {
          type: 'button',
          class: 'bouton',
          'data-action': 'copier',
          onclick: async () => {
            try {
              await copier(await carnet.markdown(genre, id));
              notifier('Copié en markdown.');
            } catch (e) {
              notifier(message(e), 'erreur');
            }
          },
        },
        'Copier',
      ),
      h(
        'button',
        {
          type: 'button',
          class: 'bouton',
          'data-action': 'exporter',
          onclick: async () => {
            try {
              const chemin = await carnet.exporter(genre, id);
              if (chemin) notifier(`Exporté : ${chemin}`);
            } catch (e) {
              notifier(message(e), 'erreur');
            }
          },
        },
        'Exporter',
      ),
      h('button', { type: 'button', class: 'bouton danger', 'data-action': 'supprimer', onclick: () => confirmer() }, 'Supprimer'),
    );
    const confirmer = () => {
      const annuler = h('button', { type: 'button', class: 'bouton', 'data-action': 'annuler', onclick: () => fermer() }, 'Annuler');
      const confirmation = h(
        'div',
        { class: 'confirmation', role: 'alertdialog', 'aria-label': 'Confirmer la suppression' },
        h(
          'p',
          {},
          h('strong', {}, `Supprimer « ${nom} » ?`),
          ' ',
          genre === 'reunion' ? 'Transcription, notes et compte rendu seront effacés du disque.' : 'Cette conversation sera effacée du disque.',
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
                  notifier(genre === 'reunion' ? 'Réunion supprimée.' : 'Conversation supprimée.');
                  pageVide();
                  await rafraichir();
                } catch (e) {
                  notifier(message(e), 'erreur');
                  fermer();
                }
              },
            },
            'Supprimer définitivement',
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

  async function copier(texte: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(texte);
    } catch {
      // WebKitGTK ou permissions : repli sur la sélection d'un champ caché.
      const zone = h('textarea', { class: 'presse-papiers', 'aria-hidden': 'true' });
      zone.value = texte;
      document.body.append(zone);
      zone.select();
      const ok = document.execCommand('copy');
      zone.remove();
      if (!ok) throw new Error('Copie refusée par le système.');
    }
  }

  function entete(surtitre: string, titre: string, genre: Genre, id: string, animer: boolean, enCours = false): HTMLElement {
    const h1 = h('h1', { class: 'titre' });
    if (animer) poserTitre(h1, titre);
    else {
      h1.textContent = titre;
      h1.setAttribute('aria-label', titre);
    }
    return h(
      'header',
      { class: 'entete' },
      h('p', { class: 'surtitre' }, enCours && h('span', { class: 'rec' }, 'REC'), surtitre),
      h1,
      actions(genre, id, titre),
    );
  }

  function section(titre: string, cle: string, texte: string, mot: string): HTMLElement | null {
    if (!texte.trim()) return null;
    return h('section', { class: 'bloc', 'data-section': cle }, h('h3', { class: 'bloc-titre' }, titre), markdown(texte, mot));
  }

  function pageReunion(r: ReunionLue, cible: Cible, animer: boolean): void {
    const mot = etat.resultats !== null ? etat.requete : '';
    const titre = titreReunion(r.resume);
    const jour = r.resume.debut_ms !== null ? cleDuJour(r.resume.debut_ms) : r.resume.date;
    const surtitre = [
      'Réunion',
      jour ? libelleDuJour(jour) : null,
      r.resume.debut_ms !== null ? heure(r.resume.debut_ms) + (r.resume.fin_ms !== null ? `–${heure(r.resume.fin_ms)}` : '') : null,
      r.resume.duree_s > 0 ? duree(r.resume.duree_s) : null,
    ]
      .filter(Boolean)
      .join(' · ');

    const contenu = h('div', { class: 'contenu' });
    const onglets = h('div', { class: 'onglets', role: 'tablist', 'aria-label': 'Parties de la réunion' });
    const indicateur = h('span', { class: 'onglet-indicateur', 'aria-hidden': 'true' });
    const liste: Array<[Onglet, string, number | null]> = [
      ['compte-rendu', 'Compte rendu', null],
      ['transcription', 'Transcription', r.segments.length],
      ['notes', 'Notes', null],
    ];
    const montrer = (o: Onglet, defiler?: Cible) => {
      etat.onglet = o;
      for (const b of onglets.querySelectorAll<HTMLElement>('[role="tab"]')) {
        const actif = b.dataset.onglet === o;
        b.setAttribute('aria-selected', String(actif));
        b.tabIndex = actif ? 0 : -1;
        if (actif) {
          indicateur.style.setProperty('--x', `${b.offsetLeft}px`);
          indicateur.style.setProperty('--l', `${b.offsetWidth}px`);
        }
      }
      contenu.dataset.onglet = o;
      contenu.replaceChildren(o === 'compte-rendu' ? compteRendu(r, mot) : o === 'transcription' ? transcription(r, mot) : notes(r, mot));
      if (defiler) allerA(contenu, defiler);
    };
    for (const [o, libelle, n] of liste) {
      onglets.append(
        h(
          'button',
          { type: 'button', role: 'tab', class: 'onglet', 'data-onglet': o, onclick: () => montrer(o) },
          libelle,
          n !== null && n > 0 && h('span', { class: 'nombre' }, String(n)),
        ),
      );
    }
    onglets.append(indicateur);
    onglets.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      const i = liste.findIndex(([o]) => o === etat.onglet);
      const suivant = liste[(i + (e.key === 'ArrowRight' ? 1 : liste.length - 1)) % liste.length]![0];
      montrer(suivant);
      onglets.querySelector<HTMLElement>(`[data-onglet="${suivant}"]`)?.focus();
    });

    const feuille = h('article', { class: 'feuille', 'data-genre': 'reunion', 'data-id': r.resume.nom }, entete(surtitre, titre, 'reunion', r.resume.nom, animer, r.resume.en_cours), onglets, contenu);
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
        h('p', {}, r.resume.en_cours ? 'La réunion est en cours : le compte rendu sera écrit à la fin.' : 'Pas de compte rendu pour cette réunion.'),
        r.segments.length > 0 && h('p', {}, 'La transcription est complète, dans l’onglet voisin.'),
      );
    }
    const blocs = [
      section('Résumé', 'resume', cr.resume, mot),
      section('Décisions', 'decisions', cr.decisions, mot),
      section('Actions à faire', 'actions', cr.actions, mot),
      section('Questions ouvertes', 'questions', cr.questions, mot),
      ...cr.autres.map(([t, texte]) => section(t, 'autre', texte, mot)),
    ].filter((b): b is HTMLElement => b !== null);
    if (blocs.length === 0) blocs.push(h('section', { class: 'bloc' }, markdown(cr.brut, mot)));
    blocs.forEach((b, i) => b.style.setProperty('--i', String(i)));
    return h('div', { class: 'compte-rendu' }, ...blocs);
  }

  function transcription(r: ReunionLue, mot: string): HTMLElement {
    if (r.segments.length === 0) return h('div', { class: 'vide-onglet' }, h('p', {}, 'Aucune parole transcrite.'));
    return h(
      'ol',
      { class: 'transcription' },
      ...r.segments.map((s, i) =>
        h(
          'li',
          { class: 'parole', 'data-source': s.source, 'data-start': String(s.start), style: `--i:${Math.min(i, 30)}` },
          h('span', { class: 'temps' }, horodatage(s.start)),
          h('span', { class: 'qui' }, s.source === 'moi' ? 'Moi' : 'Eux'),
          h('span', { class: 'dit' }, ...surligner(s.text, mot)),
        ),
      ),
    );
  }

  function notes(r: ReunionLue, mot: string): HTMLElement {
    if (!r.notes.trim()) return h('div', { class: 'vide-onglet' }, h('p', {}, 'Pas de notes pour cette réunion.'));
    return h('div', { class: 'notes bloc' }, markdown(r.notes, mot));
  }

  function pageConversation(c: ConversationLue, cible: Cible, animer: boolean): void {
    const mot = etat.resultats !== null ? etat.requete : '';
    const jour = c.resume.debut_ms !== null ? cleDuJour(c.resume.debut_ms) : c.resume.date;
    const surtitre = ['Conversation', libelleDuJour(jour), c.resume.debut_ms !== null ? heure(c.resume.debut_ms) : null, `${c.resume.tours} ${c.resume.tours > 1 ? 'échanges' : 'échange'}`]
      .filter(Boolean)
      .join(' · ');
    const contenu = h(
      'ol',
      { class: 'tours' },
      ...c.tours.map((t, i) =>
        h(
          'li',
          { class: 'tour', 'data-qui': t.qui, 'data-at': String(t.at_ms), style: `--i:${Math.min(i, 30)}` },
          h('span', { class: 'tour-tete' }, t.qui === 'moi' ? 'Moi' : 'Kobi', ` · ${heure(t.at_ms)}`, t.voix && h('span', { class: 'voix', title: 'À la voix' }, ' · à la voix')),
          t.qui === 'kobi' ? h('div', { class: 'tour-texte' }, markdown(t.texte, mot)) : h('p', { class: 'tour-texte' }, ...surligner(t.texte, mot)),
        ),
      ),
    );
    const feuille = h(
      'article',
      { class: 'feuille', 'data-genre': 'conversation', 'data-id': c.resume.id },
      entete(surtitre, titreConversation(c.resume.apercu), 'conversation', c.resume.id, animer),
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
    grandsYeux?.run(false);
    etat.selection = { panneau: 'stockage' };
    marquerSelection();
    const corps = h('div', { class: 'panneau', 'data-panneau': STOCKAGE.id });
    const titre = h('h1', { class: 'titre' });
    page.replaceChildren(
      h(
        'article',
        { class: 'feuille' },
        h('header', { class: 'entete' }, h('p', { class: 'surtitre' }, 'Réglages · ', STOCKAGE.sousTitre), titre),
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

  window.carnet = { rafraichir, ouvrir, normaliser };

  pageVide();
  if (disponible()) void rafraichir();
  else notifier('Ouvrez le Carnet depuis Kobi.', 'erreur');
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
