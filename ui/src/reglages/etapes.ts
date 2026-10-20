// Panneaux du premier lancement (KOB-144), réutilisables dans les Réglages :
// Claude Code, micro, modèle de transcription, dossiers de code, essai
// « Kobi, bonjour », raccourcis (le démarrage avec le système est dans
// `demarrage.ts`).
//
// Certaines commandes arrivent avec d'autres tickets (dossiers de code :
// KOB-138, démarrage : KOB-143) : sans elles, le panneau le dit et l'étape
// reste « en attente », sans bloquer le parcours.

import { h } from '../dom';
import { t } from '../i18n';
import { absente, ecouter, invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

// ----- Outils -----

/** « 453 Mo », « 1,2 Go ». */
export function octets(n: number): string {
  const mo = n / 2 ** 20;
  if (mo >= 1024) return t('reglages.etapes.go', { n: Math.round((mo / 1024) * 10) / 10 });
  return t('reglages.etapes.mo', { n: String(Math.round(mo)) });
}

/** Le texte de `cle`, ses `{nom}` remplacés par des nœuds (code, touches…). */
function avec(cle: string, noeuds: Record<string, Node>, vars: Record<string, string | number> = {}): Array<Node | string> {
  return t(cle, vars)
    .split(/\{(\w+)\}/)
    .map((x, i) => (i % 2 ? (noeuds[x] ?? `{${x}}`) : x))
    .filter((x) => x !== '');
}

import { touches } from '../touches';

export { touches };

function clavier(accelerateur: string): HTMLElement {
  const t = touches(accelerateur);
  return h('span', { class: 'touches', 'aria-label': t.join(' + ') }, ...t.flatMap((k, i) => [i > 0 ? h('span', { class: 'plus', 'aria-hidden': 'true' }, '+') : null, h('kbd', {}, k)]));
}

function etat(statut: 'ok' | 'attention' | 'erreur' | 'attente' | 'cours', texte: string, ...enfants: Array<Node | string | null>): HTMLElement {
  return h('div', { class: 'etat', 'data-statut': statut, role: 'status' }, h('span', { class: 'pastille', 'aria-hidden': 'true' }), h('div', { class: 'etat-corps' }, h('p', { class: 'etat-texte' }, texte), ...enfants));
}

function bouton(libelle: string, action: string, onclick: () => void, plein = false): HTMLButtonElement {
  return h('button', { type: 'button', class: plein ? 'bouton plein' : 'bouton', 'data-action': action, onclick }, libelle);
}

// ----- Claude Code -----

type EtatClaude =
  | { etat: 'pret'; version: string; programme: string }
  | { etat: 'non-connecte'; version: string; programme: string }
  | { etat: 'introuvable' }
  | { etat: 'casse'; programme: string; raison: string };

export const CLAUDE: Panneau = {
  id: 'claude',
  get titre() {
    return t('reglages.etapes.claudeTitre');
  },
  get sousTitre() {
    return t('reglages.etapes.claudeSousTitre');
  },
  monter(el, ctx) {
    const zone = h('div');
    const verifier = async () => {
      zone.replaceChildren(etat('cours', t('reglages.etapes.claudeRecherche')));
      ctx.signaler?.('attente', t('reglages.etapes.claudeResumeVerification'));
      let e: EtatClaude;
      try {
        e = await invoke<EtatClaude>('etat_claude');
      } catch (err) {
        zone.replaceChildren(etat('erreur', message(err)));
        ctx.signaler?.('erreur', t('reglages.etapes.resumeErreur'));
        return;
      }
      const revoir = bouton(t('reglages.etapes.claudeReverifier'), 'reverifier', () => void verifier());
      if (e.etat === 'pret') {
        zone.replaceChildren(etat('ok', t('reglages.etapes.claudePret', { version: e.version.replace(/\s*\(Claude Code\)/, '') }), h('p', { class: 'aide' }, h('code', {}, e.programme))));
        ctx.signaler?.('ok', t('reglages.etapes.claudeResumeConnecte'));
      } else if (e.etat === 'non-connecte') {
        zone.replaceChildren(
          etat('attention', t('reglages.etapes.claudeNonConnecte'), h('p', { class: 'aide' }, ...avec('reglages.etapes.claudeNonConnecteAide', { commande: h('code', {}, 'claude auth login') })), revoir),
        );
        ctx.signaler?.('attention', t('reglages.etapes.claudeResumeAConnecter'));
      } else if (e.etat === 'introuvable') {
        zone.replaceChildren(
          etat('erreur', t('reglages.etapes.claudeIntrouvable'), h('p', { class: 'aide' }, ...avec('reglages.etapes.claudeIntrouvableAide', { commande: h('code', {}, 'claude auth login') })), revoir),
        );
        ctx.signaler?.('erreur', t('reglages.etapes.claudeResumeIntrouvable'));
      } else {
        zone.replaceChildren(etat('erreur', t('reglages.etapes.claudeCasse', { raison: e.raison }), h('p', { class: 'aide' }, h('code', {}, e.programme)), revoir));
        ctx.signaler?.('erreur', t('reglages.etapes.claudeResumeCasse'));
      }
    };
    el.replaceChildren(h('p', { class: 'intro' }, t('reglages.etapes.claudeIntro')), zone);
    void verifier();
  },
};

// ----- Micro -----

interface Micros {
  micros: Array<{ nom: string; defaut: boolean }>;
  choisi: string | null;
  erreur: string | null;
}

export const MICRO: Panneau = {
  id: 'micro',
  get titre() {
    return t('reglages.etapes.microTitre');
  },
  get sousTitre() {
    return t('reglages.etapes.microSousTitre');
  },
  monter(el, ctx) {
    el.replaceChildren(h('p', { class: 'intro' }, t('reglages.etapes.microIntro')), etat('cours', t('reglages.etapes.microListe')));
    invoke<Micros>('micros').then(
      (m) => {
        const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': t('reglages.etapes.microTitre') });
        const options: Array<{ nom: string | null; libelle: string; detail?: string }> = [
          { nom: null, libelle: t('reglages.etapes.microSysteme'), detail: m.micros.find((x) => x.defaut)?.nom ?? t('reglages.etapes.microParDefaut') },
          ...m.micros.map((x) => ({ nom: x.nom, libelle: x.nom })),
        ];
        const selection = (nom: string | null) => {
          for (const b of choix.children) b.setAttribute('aria-checked', String((b.getAttribute('data-nom') || null) === nom));
        };
        for (const o of options) {
          choix.append(
            h(
              'button',
              {
                type: 'button',
                role: 'radio',
                class: 'option',
                'data-nom': o.nom ?? '',
                onclick: async () => {
                  try {
                    await invoke('choisir_micro', { nom: o.nom });
                    selection(o.nom);
                    ctx.notifier(o.nom ? t('reglages.etapes.microNotifChoisi', { nom: o.nom }) : t('reglages.etapes.microNotifSysteme'));
                    ctx.signaler?.('ok', o.nom ?? t('reglages.etapes.microResumeSysteme'));
                  } catch (e) {
                    ctx.notifier(message(e), 'erreur');
                  }
                },
              },
              h('span', { class: 'option-rond', 'aria-hidden': 'true' }),
              h('span', { class: 'option-corps' }, h('span', { class: 'option-titre' }, o.libelle), o.detail && h('span', { class: 'option-detail' }, o.detail)),
            ),
          );
        }
        selection(m.choisi);
        const rien = m.micros.length === 0;
        el.replaceChildren(
          h('p', { class: 'intro' }, t('reglages.etapes.microIntro')),
          rien
            ? etat('attention', t('reglages.etapes.microAucun'), h('p', { class: 'aide' }, m.erreur ?? t('reglages.etapes.microAucunAide')))
            : choix,
        );
        ctx.signaler?.(rien ? 'attention' : 'ok', rien ? t('reglages.etapes.microResumeAucun') : (m.choisi ?? t('reglages.etapes.microResumeSysteme')));
      },
      (e) => {
        el.replaceChildren(etat('erreur', message(e)));
        ctx.signaler?.('erreur', t('reglages.etapes.resumeErreur'));
      },
    );
  },
};

// ----- Modèle de transcription -----

interface EtatModele {
  dossier: string | null;
  nom: string;
  taille: number;
  en_cours: boolean;
}

export const MODELE: Panneau = {
  id: 'modele',
  get titre() {
    return t('reglages.etapes.modeleTitre');
  },
  get sousTitre() {
    return t('reglages.etapes.modeleSousTitre');
  },
  monter(el, ctx) {
    const zone = h('div');
    const barre = h('div', { class: 'barre', role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-label': t('reglages.etapes.modeleBarre') }, h('span', { class: 'barre-plein' }));
    const legende = h('p', { class: 'aide legende' });
    let taille = 0;
    const enCours = () => {
      zone.replaceChildren(
        etat('cours', t('reglages.etapes.modeleTelechargement'), barre, legende, bouton(t('reglages.etapes.annuler'), 'annuler', () => void invoke('annuler_telechargement'))),
      );
      ctx.signaler?.('attente', t('reglages.etapes.modeleResumeTelechargement'));
    };
    const installe = (dossier: string) => {
      zone.replaceChildren(etat('ok', t('reglages.etapes.modeleInstalle'), h('p', { class: 'aide' }, h('code', {}, dossier))));
      ctx.signaler?.('ok', t('reglages.etapes.modeleResumeInstalle'));
    };
    const proposer = (erreur?: string) => {
      zone.replaceChildren(
        etat(
          erreur ? 'erreur' : 'attention',
          erreur ? t('reglages.etapes.modeleArrete', { erreur }) : t('reglages.etapes.modeleAProposer', { taille: octets(taille) }),
          h('p', { class: 'aide' }, t('reglages.etapes.modeleAide')),
          bouton(erreur ? t('reglages.etapes.modeleReprendre') : t('reglages.etapes.modeleTelecharger', { taille: octets(taille) }), 'telecharger', () => {
            enCours();
            invoke('telecharger_modele').catch((e) => proposer(message(e)));
          }, true),
        ),
      );
      ctx.signaler?.(erreur ? 'erreur' : 'attention', t('reglages.etapes.modeleResumeATelecharger'));
    };
    const arrets = [
      ecouter<{ etape: string; recu: number; total: number }>('modele://progression', (p) => {
        if (!zone.contains(barre)) enCours();
        const pc = p.total ? Math.floor((p.recu / p.total) * 100) : 0;
        barre.style.setProperty('--p', String(pc / 100));
        barre.setAttribute('aria-valuenow', String(pc));
        legende.textContent = p.etape === 'extraction' ? t('reglages.etapes.modeleExtraction', { pc }) : t('reglages.etapes.modeleProgression', { recu: octets(p.recu), total: octets(p.total), pc });
      }),
      ecouter<string>('modele://fini', (dossier) => {
        installe(dossier);
        ctx.notifier(t('reglages.etapes.modeleNotifInstalle'));
      }),
      ecouter<string>('modele://erreur', (e) => proposer(e)),
    ];
    el.replaceChildren(h('p', { class: 'intro' }, t('reglages.etapes.modeleIntro')), zone);
    zone.replaceChildren(etat('cours', t('reglages.etapes.modeleRecherche')));
    invoke<EtatModele>('etat_modele').then(
      (m) => {
        taille = m.taille;
        if (m.dossier) installe(m.dossier);
        else if (m.en_cours) enCours();
        else proposer();
      },
      (e) => zone.replaceChildren(etat('erreur', message(e))),
    );
    return () => arrets.forEach((a) => a());
  },
};

// ----- Dossiers de code -----

/** Dépôt git trouvé sous un dossier (`lister_depots`, KOB-153). */
interface Depot {
  nom: string;
  chemin: string;
  branche: string | null;
  langage: string | null;
}

const sansBarre = (chemin: string) => chemin.replace(/[\\/]+$/, '');

export const DOSSIERS: Panneau = {
  id: 'dossiers-code',
  get titre() {
    return t('reglages.etapes.dossiersTitre');
  },
  get sousTitre() {
    return t('reglages.etapes.dossiersSousTitre');
  },
  monter(el, ctx) {
    const zone = h('div');
    el.replaceChildren(h('p', { class: 'intro' }, t('reglages.etapes.dossiersIntro')), zone);
    let dossiers: string[] = [];
    const dessiner = () => {
      const liste = h(
        'ul',
        { class: 'dossiers' },
        ...dossiers.map((d) =>
          h(
            'li',
            { class: 'dossier-code' },
            h('code', { title: d }, d),
            h('button', { type: 'button', class: 'retirer', 'aria-label': t('reglages.etapes.dossiersRetirer', { dossier: d }), onclick: () => void enregistrer(dossiers.filter((x) => x !== d)) }, '×'),
          ),
        ),
      );
      zone.replaceChildren(
        dossiers.length ? liste : h('p', { class: 'aide' }, t('reglages.etapes.dossiersAucun')),
        h('div', { class: 'boutons' }, bouton(t('reglages.etapes.dossiersAjouter'), 'ajouter', () => void ajouter(), true)),
      );
      ctx.signaler?.(dossiers.length ? 'ok' : 'attention', dossiers.length ? t('reglages.etapes.dossiersResumeNombre', { n: dossiers.length }) : t('reglages.etapes.dossiersResumeAucun'));
    };
    const enregistrer = async (nouveaux: string[]) => {
      try {
        await invoke('set_codebases', { dossiers: nouveaux });
        dossiers = nouveaux;
        dessiner();
        ctx.notifier(t('reglages.etapes.dossiersNotifEnregistres'));
      } catch (e) {
        ctx.notifier(message(e), 'erreur');
      }
    };
    const ajouter = async () => {
      let d: string | null;
      try {
        d = await invoke<string | null>('choisir_dossier', { titre: t('reglages.etapes.dossiersDialogue') });
      } catch (e) {
        ctx.notifier(message(e), 'erreur');
        return;
      }
      if (!d) return;
      // Plusieurs dépôts dans le dossier choisi : on propose de les cocher un
      // par un (KOB-153). Le dossier est lui-même un dépôt, n'en contient
      // qu'un ou Kobi ne sait pas encore les lister : il est ajouté tel quel.
      let depots: Depot[] = [];
      try {
        depots = await invoke<Depot[]>('lister_depots', { dossier: d });
      } catch (e) {
        if (!absente(e)) ctx.notifier(message(e), 'erreur');
      }
      const dessous = depots.filter((x) => sansBarre(x.chemin) !== sansBarre(d));
      if (dessous.length < 2 || dessous.length !== depots.length) {
        if (!dossiers.includes(d)) await enregistrer([...dossiers, d]);
        return;
      }
      proposer(d, dessous);
    };
    /** Liste des dépôts trouvés, tous cochés : Kobi lira ceux qu'on garde. */
    const proposer = (racine: string, depots: Depot[]) => {
      const cases = depots.map((dep) =>
        h('input', { type: 'checkbox', checked: !dossiers.includes(dep.chemin), disabled: dossiers.includes(dep.chemin), 'data-chemin': dep.chemin }),
      );
      const valider = bouton('', 'ajouter-depots', () => {
        const choisis = cases.filter((c) => c.checked && !c.disabled).map((c) => c.dataset.chemin!);
        void enregistrer([...dossiers, ...choisis]);
      }, true);
      const compter = () => {
        const n = cases.filter((c) => c.checked && !c.disabled).length;
        valider.textContent = n ? t('reglages.etapes.depotsAjouter', { n }) : t('reglages.etapes.depotsAucunChoisi');
        valider.disabled = n === 0;
      };
      cases.forEach((c) => c.addEventListener('change', compter));
      compter();
      zone.replaceChildren(
        h('p', { class: 'aide' }, ...avec('reglages.etapes.depotsTrouves', { dossier: h('code', {}, racine) }, { n: depots.length })),
        h(
          'ul',
          { class: 'depots', 'aria-label': t('reglages.etapes.depotsListe') },
          ...depots.map((dep, i) =>
            h(
              'li',
              { class: 'depot', style: `--i:${Math.min(i, 20)}` },
              h(
                'label',
                { title: dep.chemin },
                cases[i]!,
                h('span', { class: 'depot-nom' }, dep.nom),
                h('span', { class: 'depot-infos' }, [dep.branche, dep.langage].filter(Boolean).join(' · ') || t('reglages.etapes.depotGit')),
                dossiers.includes(dep.chemin) ? h('span', { class: 'depot-infos' }, t('reglages.etapes.depotDejaLu')) : null,
              ),
            ),
          ),
        ),
        h(
          'div',
          { class: 'boutons' },
          valider,
          bouton(t('reglages.etapes.depotsToutLeDossier'), 'tout-le-dossier', () => void enregistrer(dossiers.includes(racine) ? dossiers : [...dossiers, racine])),
          bouton(t('reglages.etapes.annuler'), 'annuler', dessiner),
        ),
      );
    };
    invoke<string[]>('get_codebases').then(
      (d) => {
        dossiers = d;
        dessiner();
      },
      (e) => {
        if (absente(e)) {
          zone.replaceChildren(etat('attente', t('reglages.etapes.dossiersBientot'), h('p', { class: 'aide' }, t('reglages.etapes.dossiersBientotAide'))));
          ctx.signaler?.('attente', t('reglages.etapes.dossiersResumeBientot'));
        } else {
          zone.replaceChildren(etat('erreur', message(e)));
          ctx.signaler?.('erreur', t('reglages.etapes.resumeErreur'));
        }
      },
    );
  },
};

// ----- Raccourcis -----

interface Raccourcis {
  parler: string;
  reunion: string;
  /** Écrire à Kobi (KOB-147) ; absent d'un Kobi plus ancien. */
  ecrire?: string;
  /** Conversation en grand (KOB-169) ; absent d'un Kobi plus ancien. */
  agrandir?: string;
}

export const RACCOURCIS: Panneau = {
  id: 'raccourcis',
  get titre() {
    return t('reglages.etapes.raccourcisTitre');
  },
  get sousTitre() {
    return t('reglages.etapes.raccourcisSousTitre');
  },
  monter(el, ctx) {
    el.replaceChildren(etat('cours', t('reglages.etapes.unInstant')));
    invoke<Raccourcis>('raccourcis').then(
      (r) => {
        // Tous les raccourcis globaux de Kobi (KOB-183), lus dans ses réglages.
        const lignes: Array<[string | undefined, string, string]> = [
          [r.parler, t('reglages.etapes.raccourciParler'), t('reglages.etapes.raccourciParlerAide')],
          [r.ecrire, t('reglages.etapes.raccourciEcrire'), t('reglages.etapes.raccourciEcrireAide')],
          [r.reunion, t('reglages.etapes.raccourciReunion'), t('reglages.etapes.raccourciReunionAide')],
          [r.agrandir, t('reglages.etapes.raccourciAgrandir'), t('reglages.etapes.raccourciAgrandirAide')],
        ];
        const presents = lignes.filter((l): l is [string, string, string] => Boolean(l[0]));
        const nombre = ['', 'raccourcisUn', 'raccourcisDeux', 'raccourcisTrois', 'raccourcisQuatre'][presents.length];
        const intro = t(`reglages.etapes.${nombre || 'raccourcisCes'}`);
        el.replaceChildren(
          h('p', { class: 'intro' }, t('reglages.etapes.raccourcisIntro', { nombre: intro })),
          h('dl', { class: 'raccourcis' }, ...presents.flatMap(([touches, titre, aide]) => [h('dt', {}, clavier(touches)), h('dd', {}, h('strong', {}, titre), h('br'), aide)])),
          h('p', { class: 'aide' }, t('reglages.etapes.raccourcisMenu')),
        );
        ctx.signaler?.('ok', touches(r.parler).join('+'));
      },
      (e) => el.replaceChildren(etat('erreur', message(e))),
    );
  },
};

// ----- Essai -----

/** « Kobi, bonjour » : on attend la question transcrite puis la réponse. */
export function monterEssai(el: HTMLElement, ctx: Contexte, parler: string): () => void {
  const question = h('p', { class: 'essai-question' });
  const reponse = h('p', { class: 'essai-reponse' });
  const zone = h('div', { class: 'essai-etat' });
  let minuterie = 0;
  let entendu = false;
  const attendre = () => {
    entendu = false;
    question.textContent = '';
    reponse.textContent = '';
    zone.replaceChildren(etat('cours', t('reglages.etapes.essaiEcoute')));
    ctx.signaler?.('attente', t('reglages.etapes.essaiResumeAEssayer'));
    window.clearTimeout(minuterie);
    minuterie = window.setTimeout(() => {
      if (entendu) return;
      zone.replaceChildren(
        etat(
          'attention',
          t('reglages.etapes.essaiRienEntendu'),
          h('p', { class: 'aide' }, ...avec('reglages.etapes.essaiRienEntenduAide', { touches: clavier(parler) })),
          bouton(t('reglages.etapes.essaiRecommencer'), 'recommencer', attendre),
        ),
      );
      ctx.signaler?.('attention', t('reglages.etapes.essaiResumeRienEntendu'));
    }, 20_000);
  };
  const arrets = [
    ecouter<string>('question', (q) => {
      entendu = true;
      question.textContent = t('reglages.etapes.essaiQuestion', { question: q });
      zone.replaceChildren(etat('cours', t('reglages.etapes.essaiReflechit')));
    }),
    ecouter<string>('answer', (a) => {
      entendu = true;
      reponse.textContent = a;
      zone.replaceChildren(etat('ok', t('reglages.etapes.essaiReussi')));
      ctx.signaler?.('ok', t('reglages.etapes.essaiResumeReussi'));
    }),
    ecouter<{ title: string; message: string }>('erreur', (e) => {
      entendu = true;
      zone.replaceChildren(etat('erreur', e.title, h('p', { class: 'aide' }, e.message), bouton(t('reglages.etapes.essaiRecommencer'), 'recommencer', attendre)));
      ctx.signaler?.('erreur', e.title);
    }),
  ];
  el.replaceChildren(
    h('p', { class: 'essai-consigne' }, ...avec('reglages.etapes.essaiConsigne', { phrase: h('strong', {}, t('reglages.etapes.essaiPhrase')) })),
    h('p', { class: 'aide' }, ...avec('reglages.etapes.essaiAide', { touches: clavier(parler) })),
    question,
    reponse,
    zone,
  );
  attendre();
  return () => {
    window.clearTimeout(minuterie);
    arrets.forEach((a) => a());
  };
}

export const ESSAI: Panneau = {
  id: 'essai',
  get titre() {
    return t('reglages.etapes.essaiTitre');
  },
  get sousTitre() {
    return t('reglages.etapes.essaiPhrase');
  },
  monter(el, ctx) {
    let arreter = () => {};
    let fini = false;
    invoke<Raccourcis>('raccourcis').then(
      (r) => {
        if (!fini) arreter = monterEssai(el, ctx, r.parler);
      },
      () => {
        if (!fini) arreter = monterEssai(el, ctx, 'Ctrl+Shift+Space');
      },
    );
    return () => {
      fini = true;
      arreter();
    };
  },
};

// ----- Rejouer -----

export const PARCOURS: Panneau = {
  id: 'premier-lancement',
  get titre() {
    return t('reglages.etapes.parcoursTitre');
  },
  get sousTitre() {
    return t('reglages.etapes.parcoursSousTitre');
  },
  monter(el, ctx) {
    el.replaceChildren(
      h('p', { class: 'intro' }, t('reglages.etapes.parcoursIntro')),
      bouton(t('reglages.etapes.parcoursRejouer'), 'rejouer', () => invoke('ouvrir_bienvenue').catch((e) => ctx.notifier(message(e), 'erreur')), true),
    );
  },
};
