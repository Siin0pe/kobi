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

const NATURES: Record<Nature, string> = { lecture: 'lit', ecriture: 'écrit', suppression: 'supprime' };

/** « 12 outils · 8 lisent · 3 écrivent · 1 supprime ». */
export function compte(outils: ReadonlyArray<{ nature: Nature }>): string {
  if (!outils.length) return 'pas encore testé';
  const n = (nature: Nature) => outils.filter((o) => o.nature === nature).length;
  const parties = [`${outils.length} outil${outils.length > 1 ? 's' : ''}`];
  const l = n('lecture');
  const e = n('ecriture');
  const s = n('suppression');
  if (l) parties.push(`${l} ${l > 1 ? 'lisent' : 'lit'}`);
  if (e) parties.push(`${e} ${e > 1 ? 'écrivent' : 'écrit'}`);
  if (s) parties.push(`${s} ${s > 1 ? 'suppriment' : 'supprime'}`);
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
    zone.replaceChildren(etat('cours', 'Je regarde les serveurs MCP de Claude Code…'));
    let candidats: Candidat[];
    try {
      candidats = await invoke<Candidat[]>('mcp_candidats');
    } catch (e) {
      zone.replaceChildren(etat(absente(e) ? 'attente' : 'erreur', absente(e) ? 'L’import arrive avec la prochaine version de Kobi.' : message(e)));
      return;
    }
    const nouveaux = candidats.filter((c) => !c.deja);
    if (!candidats.length) {
      zone.replaceChildren(etat('attention', 'Aucun serveur MCP dans Claude Code.', h('p', { class: 'aide' }, 'Ajoute-les ici à la main, ou dans Claude Code avec ', h('code', {}, 'claude mcp add'), '.')));
      return;
    }
    const coches = new Set(nouveaux.map((c) => c.nom));
    const importer = bouton('Importer', 'importer', () => void lancer(), 'plein');
    const majBouton = () => {
      importer.disabled = coches.size === 0;
      importer.textContent = coches.size ? `Importer ${coches.size} serveur${coches.size > 1 ? 's' : ''}` : 'Importer';
    };
    // Échec de l'import : affiché sous le bouton jusqu'au prochain essai,
    // le temps de lire la marche à suivre (KOB-185).
    const resultat = h('div', { class: 'mcp-import-resultat' });
    const lancer = async () => {
      const noms = [...coches];
      importer.disabled = true;
      importer.textContent = 'Import et test…';
      resultat.replaceChildren();
      try {
        const serveurs = await invoke<ServeurMcp[]>('mcp_importer', { noms });
        ctx.notifier(`${noms.length} serveur${noms.length > 1 ? 's' : ''} importé${noms.length > 1 ? 's' : ''} : ils servent dès la prochaine conversation.`);
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
            checked: !c.deja,
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
              h('span', { class: 'mcp-portee' }, c.deja ? 'déjà connecté' : c.portee),
              c.secrets.length > 0 && h('span', { class: 'puce', title: `${c.secrets.join(', ')} : rangé dans le gestionnaire d’identifiants` }, 'secret'),
            ),
          );
        }),
      ),
      nouveaux.length ? h('div', { class: 'boutons' }, importer) : h('p', { class: 'aide' }, 'Tous les serveurs de Claude Code sont déjà connectés.'),
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
  const montrerTest = (t: Test) => {
    if (t.etat === 'ok') {
      resultat.replaceChildren(
        etat('ok', `Connecté : ${compte(t.outils)}.`),
        h(
          'ul',
          { class: 'mcp-outils' },
          ...t.outils.map((o) => h('li', { class: 'mcp-outil', 'data-nature': o.nature, title: o.description }, h('code', {}, o.nom), h('span', { class: 'mcp-nature' }, NATURES[o.nature]))),
        ),
      );
    } else if (t.etat === 'autorisation') {
      resultat.replaceChildren(etat('attention', t.message, h('div', { class: 'boutons' }, bouton('Autoriser Kobi', 'autoriser', () => void autoriser(), 'plein'))));
    } else {
      resultat.replaceChildren(etat('erreur', t.message));
    }
  };
  const apresTest = async (t: Test) => {
    tests.set(s.nom, t);
    montrerTest(t);
    if (t.etat === 'ok') {
      try {
        recharger(await invoke<ServeurMcp[]>('mcp_serveurs'));
      } catch {
        /* la liste reste celle d'avant */
      }
    }
  };
  const tester = async () => {
    resultat.replaceChildren(etat('cours', 'Je me connecte et je liste les outils…'));
    try {
      await apresTest(await invoke<Test>('mcp_tester', { nom: s.nom }));
    } catch (e) {
      resultat.replaceChildren(etat('erreur', message(e)));
    }
  };
  const autoriser = async () => {
    resultat.replaceChildren(etat('cours', 'Connecte-toi dans la page qui vient de s’ouvrir dans ton navigateur…'));
    try {
      await apresTest(await invoke<Test>('mcp_autoriser', { nom: s.nom }));
    } catch (e) {
      resultat.replaceChildren(etat('erreur', message(e)));
    }
  };
  const supprimer = async () => {
    try {
      recharger(await invoke<ServeurMcp[]>('mcp_supprimer', { nom: s.nom }));
      ctx.notifier(`${s.nom} retiré, secrets effacés.`);
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
      'aria-label': `Utiliser ${s.nom}`,
      onclick: () => void regler({ actif: !s.actif }, s.actif ? `${s.nom} désactivé.` : `${s.nom} activé : il sert dès la prochaine conversation.`),
    },
    h('span', { class: 'interrupteur-rond', 'aria-hidden': 'true' }),
  );
  const dernier = tests.get(s.nom);
  if (dernier) montrerTest(dernier);
  const secrets = s.secrets.length
    ? h(
        'p',
        { class: 'aide mcp-secrets' },
        'Secrets dans le gestionnaire d’identifiants : ',
        ...s.secrets.flatMap((x, i) => [i ? ', ' : '', h('code', { 'data-present': String(x.present) }, x.nom), x.present ? '' : ' (absent)']),
      )
    : null;
  return h(
    'li',
    { class: 'mcp-serveur', 'data-nom': s.nom, 'data-actif': String(s.actif) },
    h(
      'div',
      { class: 'mcp-entete' },
      interrupteur,
      h('div', { class: 'mcp-titre' }, h('span', { class: 'mcp-nom' }, s.nom), h('span', { class: 'puce' }, s.transport === 'stdio' ? 'local' : s.transport.toUpperCase())),
      h('div', { class: 'boutons' }, bouton('Tester', 'tester', () => void tester()), bouton('Retirer', 'retirer', () => void supprimer(), 'discret')),
    ),
    h('p', { class: 'mcp-resume', title: s.resume }, s.resume),
    h('p', { class: 'aide' }, compte(s.outils), s.origine ? ` · ${s.origine}` : ''),
    secrets,
    h(
      'div',
      { class: 'mcp-ecriture' },
      h('span', { class: 'mcp-ecriture-titre' }, 'Quand il modifie ou envoie quelque chose'),
      segments<Ecriture>(
        `Écriture pour ${s.nom}`,
        [
          { valeur: 'demander', libelle: 'Toujours demander' },
          { valeur: 'sans_demander', libelle: 'Faire sans demander' },
        ],
        s.ecriture,
        (v) => void regler({ ecriture: v }, v === 'demander' ? `${s.nom} : Kobi te demandera avant d’agir.` : `${s.nom} : Kobi agira sans demander (sauf suppression).`),
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
  const nom = champ('nom', 'Nom', { placeholder: 'linear' });
  const url = champ('url', 'Adresse', { placeholder: 'https://mcp.exemple.com/mcp' });
  const entetes = zone('entetes', 'En-têtes secrets', 'Authorization: Bearer …');
  const commande = champ('commande', 'Commande', { placeholder: 'npx' });
  const args = zone('args', 'Arguments, un par ligne', '-y\nnom-du-serveur');
  const env = zone('env', 'Variables, une par ligne', 'NOTES_DIR=C:/notes');
  const secrets = zone('secrets', 'Variables secrètes, une par ligne', 'API_KEY=…');
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
      ctx.notifier(`${serveur.nom} ajouté. Teste-le pour classer ses outils.`);
      for (const c of [nom, url, entetes, commande, args, env, secrets]) c.input.value = '';
    } catch (e) {
      ctx.notifier(message(e), 'erreur');
    }
  };
  dessinerChamps();
  return h(
    'details',
    { class: 'mcp-ajout' },
    h('summary', {}, 'Ajouter un serveur à la main'),
    segments<Transport>(
      'Type de serveur',
      [
        { valeur: 'http', libelle: 'HTTP' },
        { valeur: 'sse', libelle: 'SSE' },
        { valeur: 'stdio', libelle: 'Commande locale' },
      ],
      transport,
      (t) => {
        transport = t;
        dessinerChamps();
      },
    ),
    champs,
    h('p', { class: 'aide' }, 'Les secrets vont dans le gestionnaire d’identifiants de Windows, jamais dans le fichier de réglages.'),
    h('div', { class: 'boutons' }, bouton('Ajouter', 'ajouter', () => void ajouter(), 'plein')),
  );
}

// ----- Panneaux -----

export const MCP: Panneau = {
  id: 'mcp',
  titre: 'Outils MCP',
  sousTitre: 'Linear, Notion… : lire et agir',
  monter(el, ctx) {
    const liste = h('ul', { class: 'mcp-serveurs' });
    const tests: Tests = new Map();
    const recharger = (serveurs: ServeurMcp[]) => {
      liste.replaceChildren(
        ...(serveurs.length ? serveurs.map((s) => carteServeur(s, ctx, recharger, tests)) : [h('li', { class: 'aide' }, 'Aucun serveur connecté : Kobi ne lit que ton code.')]),
      );
      const actifs = serveurs.filter((s) => s.actif).length;
      ctx.signaler?.(actifs ? 'ok' : 'attente', actifs ? `${actifs} connecté${actifs > 1 ? 's' : ''}` : 'aucun');
    };
    el.replaceChildren(
      h('p', { class: 'intro' }, 'Kobi utilise ces serveurs pour lire et agir dans tes outils. Il lit sans demander ; ce qui modifie ou envoie quelque chose passe par une carte de confirmation, sauf si tu choisis « Faire sans demander ». Une suppression est toujours demandée.'),
      h('div', { class: 'reglage', 'data-reglage': 'serveurs' }, h('div', { class: 'reglage-titre' }, 'Serveurs connectés'), liste),
      h('div', { class: 'reglage', 'data-reglage': 'import' }, h('div', { class: 'reglage-titre' }, 'Importer depuis Claude Code'), h('p', { class: 'aide' }, 'Les serveurs que tu as déjà dans Claude Code, en un clic. Ceux qui se connectent par ton compte (Linear…) gardent la connexion de Claude Code.'), blocImport(ctx, recharger)),
      h('div', { class: 'reglage', 'data-reglage': 'ajout' }, formulaireAjout(ctx, recharger)),
    );
    liste.replaceChildren(h('li', { class: 'chargement' }, 'Lecture des serveurs…'));
    invoke<ServeurMcp[]>('mcp_serveurs').then(recharger, (e) => {
      liste.replaceChildren(h('li', { class: absente(e) ? 'aide' : 'erreur' }, absente(e) ? 'Les outils MCP arrivent avec la prochaine version de Kobi.' : message(e)));
      ctx.signaler?.(absente(e) ? 'attente' : 'erreur', absente(e) ? 'bientôt' : 'erreur');
    });
  },
};

/** Étape facultative du premier lancement : l'import seul. */
export const OUTILS: Panneau = {
  id: 'outils',
  titre: 'Connecter tes outils',
  sousTitre: 'Facultatif',
  monter(el, ctx) {
    const signaler = (serveurs: ServeurMcp[]) => {
      const n = serveurs.filter((s) => s.actif).length;
      ctx.signaler?.(n ? 'ok' : 'attente', n ? `${n} connecté${n > 1 ? 's' : ''}` : 'facultatif');
    };
    el.replaceChildren(
      h('p', { class: 'intro' }, 'Kobi peut lire et agir dans tes outils (tickets, notes, agenda) par les serveurs MCP. Prends ceux que tu as déjà dans Claude Code ; il te demandera toujours avant de modifier quoi que ce soit.'),
      blocImport(ctx, signaler),
      h('p', { class: 'aide' }, 'Tu pourras en ajouter ou en retirer dans Réglages → Outils MCP.'),
    );
    invoke<ServeurMcp[]>('mcp_serveurs').then(signaler, () => ctx.signaler?.('attente', 'facultatif'));
  },
};
