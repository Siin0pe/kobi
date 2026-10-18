// Panneaux du premier lancement (KOB-144), réutilisables dans les Réglages :
// Claude Code, micro, modèle de transcription, dossiers de code, essai
// « Kobi, bonjour », raccourcis (le démarrage avec le système est dans
// `demarrage.ts`).
//
// Certaines commandes arrivent avec d'autres tickets (dossiers de code :
// KOB-138, démarrage : KOB-143) : sans elles, le panneau le dit et l'étape
// reste « en attente », sans bloquer le parcours.

import { h } from '../dom';
import { absente, ecouter, invoke, message } from '../ipc';
import type { Contexte, Panneau } from './panneau';

// ----- Outils -----

/** « 453 Mo », « 1,2 Go ». */
export function octets(n: number): string {
  const mo = n / 2 ** 20;
  if (mo >= 1024) return `${(mo / 1024).toLocaleString('fr-FR', { maximumFractionDigits: 1 })} Go`;
  return `${Math.round(mo)} Mo`;
}

/** Touches d'un raccourci Tauri (`Ctrl+Shift+Space`), en français. */
export function touches(accelerateur: string): string[] {
  const noms: Record<string, string> = { space: 'Espace', control: 'Ctrl', ctrl: 'Ctrl', commandorcontrol: 'Ctrl', shift: 'Maj', alt: 'Alt', super: 'Win', enter: 'Entrée' };
  return accelerateur.split('+').map((t) => noms[t.trim().toLowerCase()] ?? t.trim().toUpperCase());
}

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
  titre: 'Claude Code',
  sousTitre: 'Le cerveau de Kobi',
  monter(el, ctx) {
    const zone = h('div');
    const verifier = async () => {
      zone.replaceChildren(etat('cours', 'Je cherche Claude Code sur cet ordinateur…'));
      ctx.signaler?.('attente', 'vérification');
      let e: EtatClaude;
      try {
        e = await invoke<EtatClaude>('etat_claude');
      } catch (err) {
        zone.replaceChildren(etat('erreur', message(err)));
        ctx.signaler?.('erreur', 'erreur');
        return;
      }
      const revoir = bouton('Revérifier', 'reverifier', () => void verifier());
      if (e.etat === 'pret') {
        zone.replaceChildren(etat('ok', `Claude Code ${e.version.replace(/\s*\(Claude Code\)/, '')} est installé et connecté.`, h('p', { class: 'aide' }, h('code', {}, e.programme))));
        ctx.signaler?.('ok', 'connecté');
      } else if (e.etat === 'non-connecte') {
        zone.replaceChildren(
          etat('attention', 'Claude Code est installé, mais pas connecté.', h('p', { class: 'aide' }, 'Ouvre un terminal, lance ', h('code', {}, 'claude auth login'), ', puis reviens ici.'), revoir),
        );
        ctx.signaler?.('attention', 'à connecter');
      } else if (e.etat === 'introuvable') {
        zone.replaceChildren(
          etat('erreur', 'Je ne trouve pas Claude Code.', h('p', { class: 'aide' }, 'Installe-le depuis claude.ai/code (une commande dans un terminal), connecte-toi avec ', h('code', {}, 'claude auth login'), ', puis revérifie.'), revoir),
        );
        ctx.signaler?.('erreur', 'introuvable');
      } else {
        zone.replaceChildren(etat('erreur', `Claude Code ne répond pas : ${e.raison}.`, h('p', { class: 'aide' }, h('code', {}, e.programme)), revoir));
        ctx.signaler?.('erreur', 'ne répond pas');
      }
    };
    el.replaceChildren(h('p', { class: 'intro' }, 'Kobi réfléchit avec ton Claude Code : c’est lui qui répond. Il ne voit que le texte de la conversation, jamais l’audio.'), zone);
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
  titre: 'Micro',
  sousTitre: 'Celui que Kobi écoute',
  monter(el, ctx) {
    el.replaceChildren(h('p', { class: 'intro' }, 'L’écoute se fait sur cet ordinateur : le son ne part nulle part.'), etat('cours', 'Je liste les micros…'));
    invoke<Micros>('micros').then(
      (m) => {
        const choix = h('div', { class: 'choix', role: 'radiogroup', 'aria-label': 'Micro' });
        const options: Array<{ nom: string | null; libelle: string; detail?: string }> = [
          { nom: null, libelle: 'Celui du système', detail: m.micros.find((x) => x.defaut)?.nom ?? 'micro par défaut' },
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
                    ctx.notifier(o.nom ? `Micro : ${o.nom}` : 'Micro du système');
                    ctx.signaler?.('ok', o.nom ?? 'micro du système');
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
          h('p', { class: 'intro' }, 'L’écoute se fait sur cet ordinateur : le son ne part nulle part.'),
          rien
            ? etat('attention', 'Je ne trouve aucun micro.', h('p', { class: 'aide' }, m.erreur ?? 'Branche un micro : Kobi l’utilisera dès qu’il apparaît. En attendant, le clavier marche aussi.'))
            : choix,
        );
        ctx.signaler?.(rien ? 'attention' : 'ok', rien ? 'aucun micro' : (m.choisi ?? 'micro du système'));
      },
      (e) => {
        el.replaceChildren(etat('erreur', message(e)));
        ctx.signaler?.('erreur', 'erreur');
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
  titre: 'Transcription',
  sousTitre: 'Le modèle qui écrit ce qu’il entend',
  monter(el, ctx) {
    const zone = h('div');
    const barre = h('div', { class: 'barre', role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-label': 'Téléchargement du modèle' }, h('span', { class: 'barre-plein' }));
    const legende = h('p', { class: 'aide legende' });
    let taille = 0;
    const enCours = () => {
      zone.replaceChildren(
        etat('cours', 'Je télécharge le modèle…', barre, legende, bouton('Annuler', 'annuler', () => void invoke('annuler_telechargement'))),
      );
      ctx.signaler?.('attente', 'téléchargement');
    };
    const installe = (dossier: string) => {
      zone.replaceChildren(etat('ok', 'Le modèle de transcription est installé.', h('p', { class: 'aide' }, h('code', {}, dossier))));
      ctx.signaler?.('ok', 'installé');
    };
    const proposer = (erreur?: string) => {
      zone.replaceChildren(
        etat(
          erreur ? 'erreur' : 'attention',
          erreur ? `Le téléchargement s’est arrêté : ${erreur}` : `Il faut le télécharger une fois : ${octets(taille)}.`,
          h('p', { class: 'aide' }, 'Nemotron, de NVIDIA, tourne sur le processeur. Un téléchargement interrompu reprend là où il s’était arrêté.'),
          bouton(erreur ? 'Reprendre' : `Télécharger (${octets(taille)})`, 'telecharger', () => {
            enCours();
            invoke('telecharger_modele').catch((e) => proposer(message(e)));
          }, true),
        ),
      );
      ctx.signaler?.(erreur ? 'erreur' : 'attention', 'à télécharger');
    };
    const arrets = [
      ecouter<{ etape: string; recu: number; total: number }>('modele://progression', (p) => {
        if (!zone.contains(barre)) enCours();
        const pc = p.total ? Math.floor((p.recu / p.total) * 100) : 0;
        barre.style.setProperty('--p', String(pc / 100));
        barre.setAttribute('aria-valuenow', String(pc));
        legende.textContent = p.etape === 'extraction' ? `Installation… ${pc} %` : `${octets(p.recu)} sur ${octets(p.total)} · ${pc} %`;
      }),
      ecouter<string>('modele://fini', (dossier) => {
        installe(dossier);
        ctx.notifier('Modèle installé.');
      }),
      ecouter<string>('modele://erreur', (e) => proposer(e)),
    ];
    el.replaceChildren(h('p', { class: 'intro' }, 'Kobi transcrit en local : ta voix ne quitte jamais l’ordinateur.'), zone);
    zone.replaceChildren(etat('cours', 'Je regarde si le modèle est là…'));
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

export const DOSSIERS: Panneau = {
  id: 'dossiers-code',
  titre: 'Dossiers de code',
  sousTitre: 'Ce que Kobi peut lire',
  monter(el, ctx) {
    const zone = h('div');
    el.replaceChildren(h('p', { class: 'intro' }, 'Kobi lit et cherche dans ces dossiers pour répondre sur ton code. Il ne modifie rien.'), zone);
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
            h('button', { type: 'button', class: 'retirer', 'aria-label': `Retirer ${d}`, onclick: () => void enregistrer(dossiers.filter((x) => x !== d)) }, '×'),
          ),
        ),
      );
      zone.replaceChildren(
        dossiers.length ? liste : h('p', { class: 'aide' }, 'Aucun dossier pour l’instant : Kobi répondra sans regarder ton code.'),
        h('div', { class: 'boutons' }, bouton('Ajouter un dossier…', 'ajouter', () => void ajouter(), true)),
      );
      ctx.signaler?.(dossiers.length ? 'ok' : 'attention', dossiers.length ? `${dossiers.length} dossier${dossiers.length > 1 ? 's' : ''}` : 'aucun');
    };
    const enregistrer = async (nouveaux: string[]) => {
      try {
        await invoke('set_codebases', { dossiers: nouveaux });
        dossiers = nouveaux;
        dessiner();
        ctx.notifier('Dossiers enregistrés : ils servent dès la prochaine question.');
      } catch (e) {
        ctx.notifier(message(e), 'erreur');
      }
    };
    const ajouter = async () => {
      try {
        const d = await invoke<string | null>('choisir_dossier', { titre: 'Dossier de code à lire' });
        if (d && !dossiers.includes(d)) await enregistrer([...dossiers, d]);
      } catch (e) {
        ctx.notifier(message(e), 'erreur');
      }
    };
    invoke<string[]>('get_codebases').then(
      (d) => {
        dossiers = d;
        dessiner();
      },
      (e) => {
        if (absente(e)) {
          zone.replaceChildren(etat('attente', 'Le choix des dossiers arrive avec la prochaine version de Kobi.', h('p', { class: 'aide' }, 'Tu pourras le faire ici, ou dans les réglages.')));
          ctx.signaler?.('attente', 'bientôt');
        } else {
          zone.replaceChildren(etat('erreur', message(e)));
          ctx.signaler?.('erreur', 'erreur');
        }
      },
    );
  },
};

// ----- Raccourcis -----

interface Raccourcis {
  parler: string;
  reunion: string;
}

export const RACCOURCIS: Panneau = {
  id: 'raccourcis',
  titre: 'Raccourcis',
  sousTitre: 'Le clavier vaut la voix',
  monter(el, ctx) {
    el.replaceChildren(etat('cours', 'Un instant…'));
    invoke<Raccourcis>('raccourcis').then(
      (r) => {
        el.replaceChildren(
          h('p', { class: 'intro' }, 'Deux raccourcis marchent partout, même quand une autre fenêtre est devant.'),
          h(
            'dl',
            { class: 'raccourcis' },
            h('dt', {}, clavier(r.parler)),
            h('dd', {}, h('strong', {}, 'Parler à Kobi'), h('br'), 'quand dire « Kobi » n’est pas pratique.'),
            h('dt', {}, clavier(r.reunion)),
            h('dd', {}, h('strong', {}, 'Enregistrer une réunion'), h('br'), 'le même raccourci l’arrête. Jamais d’enregistrement sans toi.'),
          ),
          h('p', { class: 'aide' }, 'Clic droit sur Kobi : son menu (Carnet, réglages, pause, quitter).'),
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
    zone.replaceChildren(etat('cours', 'J’écoute…'));
    ctx.signaler?.('attente', 'à essayer');
    window.clearTimeout(minuterie);
    minuterie = window.setTimeout(() => {
      if (entendu) return;
      zone.replaceChildren(
        etat(
          'attention',
          'Je n’ai rien entendu.',
          h('p', { class: 'aide' }, 'Vérifie le micro, ou appuie sur ', clavier(parler), ' puis dis « bonjour ».'),
          bouton('Recommencer', 'recommencer', attendre),
        ),
      );
      ctx.signaler?.('attention', 'rien entendu');
    }, 20_000);
  };
  const arrets = [
    ecouter<string>('question', (q) => {
      entendu = true;
      question.textContent = `« ${q} »`;
      zone.replaceChildren(etat('cours', 'Je t’ai entendu, je réfléchis…'));
    }),
    ecouter<string>('answer', (a) => {
      entendu = true;
      reponse.textContent = a;
      zone.replaceChildren(etat('ok', 'Kobi t’a répondu. Tout marche !'));
      ctx.signaler?.('ok', 'réussi');
    }),
    ecouter<{ title: string; message: string }>('erreur', (e) => {
      entendu = true;
      zone.replaceChildren(etat('erreur', e.title, h('p', { class: 'aide' }, e.message), bouton('Recommencer', 'recommencer', attendre)));
      ctx.signaler?.('erreur', e.title);
    }),
  ];
  el.replaceChildren(
    h('p', { class: 'essai-consigne' }, 'Dis : ', h('strong', {}, '« Kobi, bonjour »')),
    h('p', { class: 'aide' }, 'Ou appuie sur ', clavier(parler), ' et dis « bonjour ».'),
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
  titre: 'Essai',
  sousTitre: '« Kobi, bonjour »',
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
  titre: 'Premier lancement',
  sousTitre: 'Rejouer le parcours guidé',
  monter(el, ctx) {
    el.replaceChildren(
      h('p', { class: 'intro' }, 'Le parcours du premier lancement vérifie Claude Code, le micro, le modèle, les dossiers de code, puis fait un essai.'),
      bouton('Rejouer le premier lancement', 'rejouer', () => invoke('ouvrir_bienvenue').catch((e) => ctx.notifier(message(e), 'erreur')), true),
    );
  },
};
