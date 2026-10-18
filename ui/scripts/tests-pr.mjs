#!/usr/bin/env node
// Mode rapide de la vérification des PR (KOB-172) : lance seulement les tests
// Playwright liés aux fichiers modifiés depuis `origin/main` (ou la base
// passée en argument), d'après la carte `tests/carte.json`. La suite complète
// reste `npm test`.
//
//   npm run test:pr              # fichiers modifiés depuis origin/main
//   npm run test:pr -- main~3    # depuis une autre base
//   npm run test:pr -- --liste   # affiche les tests choisis, sans les lancer
//
// Règles :
// - un fichier de test modifié est lancé ;
// - un fichier de `ui/src` est cherché dans la carte (motifs, du plus précis
//   au plus large) ; un fichier du « socle » (scène, mascotte, coque…) ou
//   inconnu de la carte fait lancer la suite complète, par prudence ;
// - les tests « toujours » (transparence, session) sont ajoutés à toute
//   sélection ;
// - rien sous `ui/` : aucun test d'interface.

import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const ui = join(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const liste = args.includes('--liste');
const base = args.find((a) => !a.startsWith('--')) ?? 'origin/main';

const git = (...a) => execFileSync('git', a, { cwd: ui, encoding: 'utf8' });
const racine = git('rev-parse', '--show-toplevel').trim();
// Commits de la branche, plus ce qui n'est pas encore commité.
const fichiers = new Set(
  [...git('diff', '--name-only', `${base}...HEAD`).split('\n'), ...git('diff', '--name-only', 'HEAD').split('\n')]
    .map((f) => f.trim())
    .filter(Boolean)
    .map((f) => relative(ui, join(racine, f)).replaceAll('\\', '/'))
    .filter((f) => !f.startsWith('..')),
);

const carte = JSON.parse(readFileSync(join(ui, 'tests/carte.json'), 'utf8'));

/** Motif simple : `*` dans un segment, `**` pour plusieurs segments. */
function correspond(motif, chemin) {
  const re = '^' + motif.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*\*/g, '§').replace(/\*/g, '[^/]*').replace(/§/g, '.*') + '$';
  return new RegExp(re).test(chemin);
}

const tests = new Set();
/** Ajoute tests/<nom>.spec.ts s'il existe (la carte peut nommer un test d'une autre branche). */
const ajouter = (nom) => existsSync(join(ui, `tests/${nom}.spec.ts`)) && tests.add(`tests/${nom}.spec.ts`);
let complete = null;
for (const f of fichiers) {
  if (f.startsWith('tests/') && f.endsWith('.spec.ts')) {
    if (existsSync(join(ui, f))) tests.add(f);
    continue;
  }
  if (f === 'tests/helpers.ts' || f === 'tests/carte.json' || f === 'playwright.config.ts' || f === 'package.json' || f === 'package-lock.json' || f.startsWith('vite')) {
    complete ??= f;
    continue;
  }
  if (!f.startsWith('src/') && !f.endsWith('.html')) continue;
  if (carte.socle.some((m) => correspond(m, f))) {
    complete ??= f;
    continue;
  }
  const regle = carte.regles.find((r) => r.fichiers.some((m) => correspond(m, f)));
  if (!regle) {
    complete ??= f;
    continue;
  }
  regle.tests.forEach(ajouter);
}

if (complete) {
  console.log(`Mode rapide : « ${complete} » touche le socle de l'interface (ou n'est pas dans tests/carte.json) : suite complète.`);
} else if (tests.size === 0) {
  console.log('Mode rapide : aucun fichier de ui/ modifié, aucun test d’interface à lancer.');
  process.exit(0);
} else {
  carte.toujours.forEach(ajouter);
  console.log(`Mode rapide : ${tests.size} fichiers de test pour ${fichiers.size} fichiers modifiés.`);
  for (const t of [...tests].sort()) console.log(`  ${t}`);
}
if (liste) process.exit(0);

const r = spawnSync('npx', ['playwright', 'test', ...(complete ? [] : [...tests].sort())], { cwd: ui, stdio: 'inherit', shell: process.platform === 'win32' });
process.exit(r.status ?? 1);
