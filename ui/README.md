# Interface de Kobi

Interface web (TypeScript, sans framework) servie par Tauri : la mascotte, ses bulles et ses cartes. On la développe et on la teste dans un navigateur headless, sans la fenêtre native.

```sh
npm ci
npm run dev          # http://localhost:1420/?skin=kobi&state=think&backdrop=1
                     # http://localhost:1420/reglages.html (choix du skin)
npm test             # Playwright (Chromium headless, un seul worker) sur le build de production
npm run build && node scripts/captures.mjs --skin kobi --out captures/kobi   # PNG, WebM, GIF par état + tour
npm run build && node scripts/fluidite.mjs --skin kobi                      # intervalles entre images aux 56 transitions
```

Sur une machine partagée avec des mesures de performance, les tests tournent avec un seul navigateur ; `KOBI_UI_PORT` change le port du serveur de test.

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

## Skins

| Identifiant | Skin | Rendu | Prototype |
| -- | -- | -- | -- |
| `kobi` | Kobi (défaut) | DOM + CSS, police variable Recursive, brouillage des caractères | `docs/design/mascotte/Glyphe.dc.html` |
| `gribouille` | Gribouille | SVG, filtre de turbulence animé (SMIL), stop motion | `Gribouille.dc.html` |
| `mercure` | Mercure | DOM + CSS, dégradé conique flouté sur le compositeur | `Mercure.dc.html` |
| `matrice` | Matrice | grille de 135 points, minuterie de 100 ms seulement si l'image bouge | `Matrice.dc.html` |
| `gelee` | Gelée | DOM + CSS, flous (le plus coûteux) | `Gelee.dc.html` |
| `essaim` | Essaim | canevas, 46 particules, sprites de lueur | `Essaim.dc.html` |
| `factice` | — | skin de test du contrat | — |

Le skin choisi est gardé (dans l'application : `~/Kobi/config.toml`, clé `skin` ; dans un navigateur : stockage local) et appliqué à chaud depuis la page des réglages (`reglages.html`).

## Écrire un skin

1. Un dossier `src/skins/<id>/` qui exporte une fabrique `() => Skin` ; l'enregistrer dans [`src/skins/registry.ts`](src/skins/registry.ts) et dans la liste des réglages ([`src/reglages.ts`](src/reglages.ts)). Le skin factice ([`src/skins/factice`](src/skins/factice)) est le plus petit exemple complet.
2. Dessiner dans la partie droite de la scène (x ≥ 400 px) ; préfixer classes et keyframes par l'identifiant du skin.
3. Habiller les cartes communes avec des règles `.stage[data-skin='<id>'] .card-…` ; embarquer les polices dans `src/fonts/` avec leur licence (jamais de Google Fonts).
4. Respecter `setPaused` (plus aucune animation, minuterie ni boucle) et `setReducedMotion` (expressions gardées, aucun mouvement). Une animation qui échappe au CSS (SMIL, canevas) se met en pause à la main ; une boucle de canevas le signale avec `data-animating="1"`.
5. Ajouter l'identifiant à `tests/skins.spec.ts` : le test commun vérifie les 8 états, l'absence de requête externe, la pause et « réduire les animations ».

## Polices

Recursive (SIL Open Font License 1.1, [`src/fonts/OFL-Recursive.txt`](src/fonts/OFL-Recursive.txt)), fichier variable complet (axes `wght`, `CASL`, `slnt`, `MONO`, `CRSV`), jeu latin, issu de `@fontsource-variable/recursive` 5.3.0.
