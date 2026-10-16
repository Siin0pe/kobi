# Interface de Kobi

Interface web (TypeScript, sans framework) servie par Tauri : la mascotte, ses bulles et ses cartes. On la développe et on la teste dans un navigateur headless, sans la fenêtre native.

```sh
npm ci
npm run dev          # http://localhost:1420/?skin=factice&state=think&backdrop=1
npm test             # Playwright (Chromium headless) sur le build de production
npm run build && node scripts/captures.mjs --skin factice --out captures/factice
```

Paramètres d'URL : `skin` (identifiant), `state` (un des 8 états, avec des données de démonstration), `backdrop=1` (fond du thème ; la fenêtre native est transparente), `reduced=1` ou `reduced=0` (force « réduire les animations »).

## Contrat d'un skin

Défini dans [`src/contract.ts`](src/contract.ts). Un skin dessine la mascotte et rien d'autre : il reçoit un **état** et des **données**, sans logique métier.

| État | Clé | Carte commune affichée |
| -- | -- | -- |
| Veille | `idle` | aucune |
| Écoute | `listen` | question (`question`) |
| Réfléchit | `think` | sources consultées (`sources`) |
| Note | `write` | fiche de notes (`notes`) |
| Répond | `answer` | réponse en flux (`answer`, `answerTags`) |
| Propose | `suggest` | carte d'action, Créer / Pas maintenant (`action`) |
| Fait | `done` | carte d'action confirmée (`action`) |
| Oups | `error` | carte d'erreur (`error`) |

La scène ([`src/stage.ts`](src/stage.ts)) appelle :

* `mount(host, env)` une fois ; le skin dessine dans une scène logique de 820 × 560 px, mise à l'échelle de la fenêtre. Les cartes occupent la gauche (x de 40 à 400 px) : le skin dessine à droite.
* `setState(state, data)` à chaque changement d'état, `setData(data)` quand seules les données changent (texte en flux, source trouvée).
* `setPaused(true)` quand la fenêtre est masquée : plus aucune animation ni boucle.
* `setReducedMotion(reduced)` selon le réglage système « réduire les animations ».

Le skin fournit son **thème** (`backdrop`, `ink`, `paper`, `accent`, `muted`, `danger`, `font`) que la scène applique aussi aux bulles et cartes communes. Les polices sont embarquées dans `src/fonts/`, jamais chargées depuis Internet.

Pour ajouter un skin : un dossier `src/skins/<id>/` qui exporte une fabrique `() => Skin`, enregistrée dans [`src/skins/registry.ts`](src/skins/registry.ts). Le skin factice ([`src/skins/factice`](src/skins/factice)) est le plus petit exemple complet.

## Polices

Recursive (SIL Open Font License 1.1, [`src/fonts/OFL-Recursive.txt`](src/fonts/OFL-Recursive.txt)), fichier variable complet (axes `wght`, `CASL`, `slnt`, `MONO`, `CRSV`), jeu latin, issu de `@fontsource-variable/recursive` 5.3.0.
