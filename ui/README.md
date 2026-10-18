# Interface de Kobi

Interface web (TypeScript, sans framework) servie par Tauri : la mascotte, ses bulles et ses cartes. On la développe et on la teste dans un navigateur headless, sans la fenêtre native.

```sh
npm ci
npm run dev          # http://localhost:1420/?state=think&backdrop=1
npm test             # Playwright (Chromium headless, un seul worker) sur le build de production
npm run build && node scripts/captures.mjs --out captures/kobi   # PNG, WebM, GIF par état + tour
npm run build && node scripts/fluidite.mjs                       # intervalles entre images aux 56 transitions
```

Sur une machine partagée avec des mesures de performance, les tests tournent avec un seul navigateur ; `KOBI_UI_PORT` change le port du serveur de test.

Paramètres d'URL : `state` (un des 8 états, avec des données de démonstration), `backdrop=1` (fond vert de Kobi ; la fenêtre native est transparente), `reduced=1` ou `reduced=0` (force « réduire les animations »), `discret=1` (pastille), `reunion=<titre>` (indicateur REC).

## Kobi

Kobi, style Glyphe (un visage fait de lettres en police variable Recursive, vert acide), est l'identité de la marque et la seule apparence de la mascotte (décision du 18/10/2026). Il n'y a pas de système de skins : la mascotte ([`src/mascotte/`](src/mascotte)) est un composant de la scène, porté du prototype [`docs/design/mascotte/Glyphe.dc.html`](../docs/design/mascotte/Glyphe.dc.html).

Les états et leurs données sont définis dans [`src/states.ts`](src/states.ts). La mascotte reçoit un **état**, sans logique métier ; la scène ([`src/stage.ts`](src/stage.ts)) affiche les cartes à partir des **données**.

| État | Clé | Carte affichée |
| -- | -- | -- |
| Veille | `idle` | aucune |
| Écoute | `listen` | question (`question`) |
| Réfléchit | `think` | sources consultées (`sources`) |
| Note | `write` | fiche de notes (`notes`) |
| Répond | `answer` | réponse en flux (`answer`, `answerTags`) |
| Propose | `suggest` | carte d'action, Créer / Pas maintenant (`action`) |
| Fait | `done` | carte d'action confirmée (`action`) |
| Oups | `error` | carte d'erreur (`error`) |

La scène logique fait 820 × 560 px, mise à l'échelle de la fenêtre. Les cartes occupent la gauche (x de 40 à 400 px), Kobi la droite. Les couleurs de Kobi sont des variables CSS de la scène (`--ink`, `--paper`, `--accent`…, dans [`src/styles/stage.css`](src/styles/stage.css)), reprises par les cartes.

La scène met Kobi en pause quand la fenêtre est masquée ou repliée en pastille (plus aucune animation, minuterie ni boucle), et suit le réglage système « réduire les animations » (expressions gardées, aucun mouvement).

## Polices

Recursive (SIL Open Font License 1.1, [`src/fonts/OFL-Recursive.txt`](src/fonts/OFL-Recursive.txt)), fichier variable complet (axes `wght`, `CASL`, `slnt`, `MONO`, `CRSV`), jeu latin, issu de `@fontsource-variable/recursive` 5.3.0. Embarquée, jamais chargée depuis Internet.
