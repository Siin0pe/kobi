// Panneau « Outils MCP » (KOB-155) : serveurs MCP connectés à Kobi.
// Liste, import depuis Claude Code, ajout (stdio, HTTP, SSE), activation,
// réglage d'écriture par serveur (« toujours demander » / « faire sans
// demander »), bouton « Tester » qui liste les outils et leur nature.
//
// Deux panneaux : `MCP` pour la section Réglages de l'app Kobi (KOB-159)
// et `OUTILS`, l'étape facultative « Connecter tes outils » du premier
// lancement, qui ne montre que l'import.

import './mcp.css';
import { h } from '../dom';
import { t } from '../i18n';
import { absente, invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

export type Transport = 'stdio' | 'http' | 'sse';
export type Ecriture = 'demander' | 'sans_demander';
export type Nature = 'lecture' | 'ecriture' | 'suppression';

export interface ServeurMcp {
  nom: string;
  transport: Transport;
  resume: string;
  actif: boolean;
  ecriture: Ecriture;
  origine: string | null;
  commande: string;
  args: string[];
  url: string;
  env: Record<string, string>;
  entetes: Record<string, string>;
  secrets: Array<{ nom: string; present: boolean }>;
  outils: Array<{ nom: string; nature: Nature }>;
  teste_ms: number | null;
}

export interface Candidat {
  nom: string;
  portee: string;
  transport: Transport;
  resume: string;
  deja: boolean;
  secrets: string[];
}

export type Test =
  | { etat: 'ok'; outils: Array<{ nom: string; description: string; nature: Nature }> }
  | { etat: 'autorisation'; message: string }
  | { etat: 'erreur'; message: string };

export interface NouveauServeur {
  nom: string;
  transport: Transport;
  commande: string;
  args: string[];
  url: string;
  env: Record<string, string>;
  entetes: Record<string, string>;
  secrets: Record<string, string>;
}

/** « lit », « écrit », « supprime », au rendu (la langue change à chaud). */
function nature(n: Nature): string {
  return { lecture: t('reglages.mcp.nature.lecture'), ecriture: t('reglages.mcp.nature.ecriture'), suppression: t('reglages.mcp.nature.suppression') }[n];
}

/** « 12 outils · 8 lisent · 3 écrivent · 1 supprime ». */
export function compte(outils: ReadonlyArray<{ nature: Nature }>): string {
  if (!outils.length) return t('reglages.mcp.pasTeste');
  const n = (nature: Nature) => outils.filter((o) => o.nature === nature).length;
  const parties = [t('reglages.mcp.outils', { n: outils.length })];
  const l = n('lecture');
  const e = n('ecriture');
  const s = n('suppression');
  if (l) parties.push(t('reglages.mcp.lisent', { n: l }));
  if (e) parties.push(t('reglages.mcp.ecrivent', { n: e }));
  if (s) parties.push(t('reglages.mcp.suppriment', { n: s }));
  return parties.join(' · ');
}

/** `KEY=valeur` ou `Nom: valeur`, une paire par ligne. */
export function paires(texte: string, separateur: '=' | ':'): Record<string, string> {
  const out: Record<string, string> = {};
  for (const ligne of texte.split('\n')) {
    const i = ligne.indexOf(separateur);
    if (i <= 0) continue;
    const cle = ligne.slice(0, i).trim();
    if (cle) out[cle] = ligne.slice(i + 1).trim();
  }
  return out;
}

function etat(statut: 'ok' | 'attention' | 'erreur' | 'attente' | 'cours', texte: string, ...enfants: Array<Node | string | null | false>): HTMLElement {
  return h('div', { class: 'etat', 'data-statut': statut, role: 'status' }, h('span', { class: 'pastille', 'aria-hidden': 'true' }), h('div', { class: 'etat-corps' }, h('p', { class: 'etat-texte' }, texte), ...enfants));
}

function bouton(libelle: string, action: string, onclick: () => void, classe = ''): HTMLButtonElement {
  return h('button', { type: 'button', class: `bouton ${classe}`.trim(), 'data-action': action, onclick }, libelle);
}

function segments<T extends string>(legende: string, choix: ReadonlyArray<{ valeur: T; libelle: string }>, valeur: T, changer: (v: T) => void): HTMLElement {
  const groupe = h('div', { class: 'segments', role: 'radiogroup', 'aria-label': legende });
  for (const c of choix) {
    const b = h(
      'button',
      {
        type: 'button',
        role: 'radio',
        class: 'segment',
        'data-valeur': c.valeur,
        'aria-checked': String(c.valeur === valeur),
        onclick: () => {
          if (b.getAttribute('aria-checked') === 'true') return;
          for (const x of groupe.children) x.setAttribute('aria-checked', String(x === b));
          changer(c.valeur);
        },
      },
      c.libelle,
    );
    groupe.append(b);
  }
  return groupe;
}

// ----- Import depuis Claude Code -----

/**
 * Bloc « Importer depuis Claude Code » : liste les serveurs de Claude Code
 * (déjà connectés grisés), import en un clic de ceux cochés.
 */
function blocImport(ctx: Contexte, apres: (serveurs: ServeurMcp[]) => void): HTMLElement {
  const zone = h('div', { class: 'mcp-import' });
  const charger = async () => {
    zone.replaceChildren(etat('cours', t('reglages.mcp.importChargement')));
    let candidats: Candidat[];
    try {
      candidats = await invoke<Candidat[]>('mcp_candidats');
    } catch (e) {
      zone.replaceChildren(etat(absente(e) ? 'attente' : 'erreur', absente(e) ? t('reglages.mcp.importBientot') : message(e)));
      return;
    }
    const nouveaux = candidats.filter((c) => !c.deja);
    if (!candidats.length) {
      zone.replaceChildren(etat('attention', t('reglages.mcp.importAucun'), h('p', { class: 'aide' }, t('reglages.mcp.importAucunAide'), h('code', {}, 'claude mcp add'), t('reglages.mcp.importAucunAideFin'))));
      return;
    }
    // Rien n'est coché d'office (KOB-239) : Kobi n'utilise que les serveurs
    // choisis un à un, et chaque serveur alourdit ses sessions.
    const coches = new Set<string>();
    const importer = bouton(t('reglages.mcp.importer'), 'importer', () => void lancer(), 'plein');
    const majBouton = () => {
      importer.disabled = coches.size === 0;
      importer.textContent = coches.size ? t('reglages.mcp.importerN', { n: coches.size }) : t('reglages.mcp.importer');
    };
    // Échec de l'import : affiché sous le bouton jusqu'au prochain essai,
    // le temps de lire la marche à suivre (KOB-185).
    const resultat = h('div', { class: 'mcp-import-resultat' });
    const lancer = async () => {
      const noms = [...coches];
      importer.disabled = true;
      importer.textContent = t('reglages.mcp.importEnCours');
      resultat.replaceChildren();
      try {
        const serveurs = await invoke<ServeurMcp[]>('mcp_importer', { noms });
        ctx.notifier(t('reglages.mcp.importes', { n: noms.length }));
        apres(serveurs);
        void charger();
      } catch (e) {
        resultat.replaceChildren(etat('erreur', message(e)));
        majBouton();
      }
    };
    zone.replaceChildren(
      h(
        'ul',
        { class: 'mcp-candidats' },
        ...candidats.map((c) => {
          const case_ = h('input', {
            type: 'checkbox',
            'data-nom': c.nom,
            checked: false,
            disabled: c.deja,
            onchange: () => {
              if (case_.checked) coches.add(c.nom);
              else coches.delete(c.nom);
              majBouton();
            },
          });
          return h(
            'li',
            { class: 'mcp-candidat', 'data-deja': String(c.deja) },
            h(
              'label',
              {},
              case_,
              h('span', { class: 'mcp-nom' }, c.nom),
              h('span', { class: 'mcp-resume', title: c.resume }, c.resume),
              h('span', { class: 'mcp-portee' }, c.deja ? t('reglages.mcp.dejaConnecte') : c.portee),
              c.secrets.length > 0 && h('span', { class: 'puce', title: t('reglages.mcp.secretRange', { noms: c.secrets.join(', ') }) }, t('reglages.mcp.secret')),
            ),
          );
        }),
      ),
      nouveaux.length ? h('div', { class: 'boutons' }, importer) : h('p', { class: 'aide' }, t('reglages.mcp.tousConnectes')),
      resultat,
    );
    majBouton();
  };
  void charger();
  return zone;
}

// ----- Un serveur -----

/** Dernier test de chaque serveur, gardé quand la liste se redessine. */
type Tests = Map<string, Test>;

function carteServeur(s: ServeurMcp, ctx: Contexte, recharger: (serveurs: ServeurMcp[]) => void, tests: Tests): HTMLElement {
  const resultat = h('div', { class: 'mcp-test' });
  const regler = async (changement: { actif?: boolean; ecriture?: Ecriture }, texte: string) => {
    try {
      recharger(await invoke<ServeurMcp[]>('mcp_regler', { nom: s.nom, changement }));
      ctx.notifier(texte);
    } catch (e) {
      ctx.notifier(message(e), 'erreur');
    }
  };
  const montrerTest = (test: Test) => {
    if (test.etat === 'ok') {
      resultat.replaceChildren(
        etat('ok', t('reglages.mcp.connecte', { outils: compte(test.outils) })),
        h(
          'ul',
          { class: 'mcp-outils' },
          ...test.outils.map((o) => h('li', { class: 'mcp-outil', 'data-nature': o.nature, title: o.description }, h('code', {}, o.nom), h('span', { class: 'mcp-nature' }, nature(o.nature)))),
        ),
      );
    } else if (test.etat === 'autorisation') {
      resultat.replaceChildren(etat('attention', test.message, h('div', { class: 'boutons' }, bouton(t('reglages.mcp.autoriser'), 'autoriser', () => void autoriser(), 'plein'))));
    } else {
      resultat.replaceChildren(etat('erreur', test.message));
    }
  };
  const apresTest = async (test: Test) => {
    tests.set(s.nom, test);
    montrerTest(test);
    if (test.etat === 'ok') {
      try {
        recharger(await invoke<ServeurMcp[]>('mcp_serveurs'));
      } catch {
        /* la liste reste celle d'avant */
      }
    }
  };
  const tester = async () => {
    resultat.replaceChildren(etat('cours', t('reglages.mcp.testEnCours')));
    try {
      await apresTest(await invoke<Test>('mcp_tester', { nom: s.nom }));
    } catch (e) {
      resultat.replaceChildren(etat('erreur', message(e)));
    }
  };
  const autoriser = async () => {
    resultat.replaceChildren(etat('cours', t('reglages.mcp.autorisationEnCours')));
    try {
      await apresTest(await invoke<Test>('mcp_autoriser', { nom: s.nom }));
    } catch (e) {
      resultat.replaceChildren(etat('erreur', message(e)));
    }
  };
  const supprimer = async () => {
    try {
      recharger(await invoke<ServeurMcp[]>('mcp_supprimer', { nom: s.nom }));
      ctx.notifier(t('reglages.mcp.retire', { nom: s.nom }));
    } catch (e) {
      ctx.notifier(message(e), 'erreur');
    }
  };
  const interrupteur = h(
    'button',
    {
      type: 'button',
      role: 'switch',
      class: 'interrupteur',
      'aria-checked': String(s.actif),
      'aria-label': t('reglages.mcp.utiliser', { nom: s.nom }),
      onclick: () => void regler({ actif: !s.actif }, s.actif ? t('reglages.mcp.desactive', { nom: s.nom }) : t('reglages.mcp.active', { nom: s.nom })),
    },
    h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
  );
  const dernier = tests.get(s.nom);
  if (dernier) montrerTest(dernier);
  const secrets = s.secrets.length
    ? h(
        'p',
        { class: 'aide mcp-secrets' },
        t('reglages.mcp.secrets'),
        ...s.secrets.flatMap((x, i) => [i ? ', ' : '', h('code', { 'data-present': String(x.present) }, x.nom), x.present ? '' : t('reglages.mcp.absent')]),
      )
    : null;
  return h(
    'li',
    { class: 'mcp-serveur', 'data-nom': s.nom, 'data-actif': String(s.actif) },
    h(
      'div',
      { class: 'mcp-entete' },
      interrupteur,
      h('div', { class: 'mcp-titre' }, h('span', { class: 'mcp-nom' }, s.nom), h('span', { class: 'puce' }, s.transport === 'stdio' ? t('reglages.mcp.local') : s.transport.toUpperCase())),
      h('div', { class: 'boutons' }, bouton(t('reglages.mcp.tester'), 'tester', () => void tester()), bouton(t('reglages.mcp.retirer'), 'retirer', () => void supprimer(), 'discret')),
    ),
    h('p', { class: 'mcp-resume', title: s.resume }, s.resume),
    h('p', { class: 'aide' }, compte(s.outils), s.origine ? ` · ${s.origine}` : ''),
    secrets,
    h(
      'div',
      { class: 'mcp-ecriture' },
      h('span', { class: 'mcp-ecriture-titre' }, t('reglages.mcp.ecritureTitre')),
      segments<Ecriture>(
        t('reglages.mcp.ecriturePour', { nom: s.nom }),
        [
          { valeur: 'demander', libelle: t('reglages.mcp.toujoursDemander') },
          { valeur: 'sans_demander', libelle: t('reglages.mcp.sansDemander') },
        ],
        s.ecriture,
        (v) => void regler({ ecriture: v }, v === 'demander' ? t('reglages.mcp.demandera', { nom: s.nom }) : t('reglages.mcp.agiraSansDemander', { nom: s.nom })),
      ),
    ),
    resultat,
  );
}

// ----- Ajout à la main -----

function formulaireAjout(ctx: Contexte, recharger: (serveurs: ServeurMcp[]) => void): HTMLElement {
  let transport: Transport = 'http';
  const champ = (nom: string, libelle: string, attrs: Record<string, string> = {}) => {
    const input = h('input', { type: 'text', name: nom, spellcheck: 'false', autocomplete: 'off', ...attrs });
    return { input, el: h('label', { class: 'mcp-champ', 'data-champ': nom }, h('span', {}, libelle), input) };
  };
  const zone = (nom: string, libelle: string, aide: string) => {
    const input = h('textarea', { name: nom, rows: '2', spellcheck: 'false', placeholder: aide });
    return { input, el: h('label', { class: 'mcp-champ', 'data-champ': nom }, h('span', {}, libelle), input) };
  };
  const nom = champ('nom', t('reglages.mcp.champNom'), { placeholder: 'linear' });
  const url = champ('url', t('reglages.mcp.champAdresse'), { placeholder: t('reglages.mcp.champAdresseExemple') });
  const entetes = zone('entetes', t('reglages.mcp.champEntetes'), 'Authorization: Bearer …');
  const commande = champ('commande', t('reglages.mcp.champCommande'), { placeholder: 'npx' });
  const args = zone('args', t('reglages.mcp.champArgs'), t('reglages.mcp.champArgsExemple'));
  const env = zone('env', t('reglages.mcp.champEnv'), 'NOTES_DIR=C:/notes');
  const secrets = zone('secrets', t('reglages.mcp.champSecrets'), 'API_KEY=…');
  const champs = h('div', { class: 'mcp-champs' });
  const dessinerChamps = () => {
    champs.replaceChildren(nom.el, ...(transport === 'stdio' ? [commande.el, args.el, env.el, secrets.el] : [url.el, entetes.el]));
  };
  const ajouter = async () => {
    const serveur: NouveauServeur =
      transport === 'stdio'
        ? {
            nom: nom.input.value.trim(),
            transport,
            commande: commande.input.value.trim(),
            args: args.input.value.split('\n').map((a) => a.trim()).filter(Boolean),
            url: '',
            env: paires(env.input.value, '='),
            entetes: {},
            secrets: paires(secrets.input.value, '='),
          }
        : { nom: nom.input.value.trim(), transport, commande: '', args: [], url: url.input.value.trim(), env: {}, entetes: {}, secrets: paires(entetes.input.value, ':') };
    try {
      recharger(await invoke<ServeurMcp[]>('mcp_ajouter', { serveur }));
      ctx.notifier(t('reglages.mcp.ajoute', { nom: serveur.nom }));
      for (const c of [nom, url, entetes, commande, args, env, secrets]) c.input.value = '';
    } catch (e) {
      ctx.notifier(message(e), 'erreur');
    }
  };
  dessinerChamps();
  return h(
    'details',
    { class: 'mcp-ajout' },
    h('summary', {}, t('reglages.mcp.ajouterMain')),
    segments<Transport>(
      t('reglages.mcp.typeServeur'),
      [
        { valeur: 'http', libelle: 'HTTP' },
        { valeur: 'sse', libelle: 'SSE' },
        { valeur: 'stdio', libelle: t('reglages.mcp.commandeLocale') },
      ],
      transport,
      (v) => {
        transport = v;
        dessinerChamps();
      },
    ),
    champs,
    h('p', { class: 'aide' }, t('reglages.mcp.secretsAide')),
    h('div', { class: 'boutons' }, bouton(t('reglages.mcp.ajouter'), 'ajouter', () => void ajouter(), 'plein')),
  );
}

// ----- Panneaux -----

export const MCP: Panneau = {
  id: 'mcp',
  get titre() {
    return t('reglages.mcp.titre');
  },
  get sousTitre() {
    return t('reglages.mcp.sousTitre');
  },
  monter(el, ctx) {
    const liste = h('ul', { class: 'mcp-serveurs' });
    const tests: Tests = new Map();
    const recharger = (serveurs: ServeurMcp[]) => {
      liste.replaceChildren(
        ...(serveurs.length ? serveurs.map((s) => carteServeur(s, ctx, recharger, tests)) : [h('li', { class: 'aide' }, t('reglages.mcp.aucunServeur'))]),
      );
      const actifs = serveurs.filter((s) => s.actif).length;
      ctx.signaler?.(actifs ? 'ok' : 'attente', actifs ? t('reglages.mcp.connectes', { n: actifs }) : t('reglages.mcp.resumeAucun'));
    };
    el.replaceChildren(
      h('p', { class: 'intro' }, t('reglages.mcp.intro')),
      h('div', { class: 'reglage', 'data-reglage': 'serveurs' }, h('div', { class: 'reglage-titre' }, t('reglages.mcp.serveursConnectes')), liste),
      h('div', { class: 'reglage', 'data-reglage': 'import' }, h('div', { class: 'reglage-titre' }, t('reglages.mcp.importTitre')), h('p', { class: 'aide' }, t('reglages.mcp.importAide')), blocImport(ctx, recharger)),
      h('div', { class: 'reglage', 'data-reglage': 'ajout' }, formulaireAjout(ctx, recharger)),
    );
    liste.replaceChildren(h('li', { class: 'chargement' }, t('reglages.mcp.chargement')));
    invoke<ServeurMcp[]>('mcp_serveurs').then(recharger, (e) => {
      liste.replaceChildren(h('li', { class: absente(e) ? 'aide' : 'erreur' }, absente(e) ? t('reglages.mcp.bientot') : message(e)));
      ctx.signaler?.(absente(e) ? 'attente' : 'erreur', absente(e) ? t('reglages.mcp.resumeBientot') : t('reglages.mcp.resumeErreur'));
    });
  },
};

/** Étape facultative du premier lancement : l'import seul. */
export const OUTILS: Panneau = {
  id: 'outils',
  get titre() {
    return t('reglages.mcp.outilsTitre');
  },
  get sousTitre() {
    return t('reglages.mcp.outilsSousTitre');
  },
  monter(el, ctx) {
    const signaler = (serveurs: ServeurMcp[]) => {
      const n = serveurs.filter((s) => s.actif).length;
      ctx.signaler?.(n ? 'ok' : 'attente', n ? t('reglages.mcp.connectes', { n }) : t('reglages.mcp.resumeFacultatif'));
    };
    el.replaceChildren(
      h('p', { class: 'intro' }, t('reglages.mcp.outilsIntro')),
      blocImport(ctx, signaler),
      h('p', { class: 'aide' }, t('reglages.mcp.outilsAide')),
    );
    invoke<ServeurMcp[]>('mcp_serveurs').then(signaler, () => ctx.signaler?.('attente', t('reglages.mcp.resumeFacultatif')));
  },
};
