# Interface de Kobi

Interface web (TypeScript, sans framework) servie par Tauri : la mascotte, ses bulles et ses cartes. On la développe et on la teste dans un navigateur headless, sans la fenêtre native.

```sh
npm ci
npm run dev          # http://localhost:1420/?state=think&backdrop=1
npm test             # Playwright (Chromium headless, un seul worker) sur le build de production
npm run build && node scripts/captures.mjs --out captures/kobi   # PNG, WebM, GIF par état + tour
npm run build && node scripts/fluidite.mjs [--charge]            # intervalles entre images aux 56 transitions
npm run build && node scripts/videos.mjs --out videos            # MP4 de chaque état, transition, veille, curseur, réunion
powershell -File scripts/cpu-windows.ps1 -Seconds 60             # CPU de l'app Windows lancée (kobi.exe + WebView2)
```

Sur une machine partagée avec des mesures de performance, les tests tournent avec un seul navigateur ; `KOBI_UI_PORT` change le port du serveur de test.

Paramètres d'URL : `state` (un des 8 états, avec des données de démonstration), `backdrop=1` (fond vert de Kobi ; la fenêtre native est transparente), `backdrop=sombre` ou `backdrop=clair` (bureaux de test pour la lisibilité), `reduced=1` ou `reduced=0` (force « réduire les animations »), `discret=1` (pastille), `reunion=<titre>` (indicateur REC).

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

## Animations

Kobi est fait de calques imbriqués, chacun avec son propre mouvement ([`src/mascotte/kobi.ts`](src/mascotte/kobi.ts)) : penché vers le curseur, volume du micro, gestes ponctuels, inertie, boucle CSS de l'état, regard, yeux.

* **Transitions** : à chaque changement d'état, le corps s'écrase (anticipation), les yeux se ferment en fente pendant que les symboles défilent, l'effet de l'ancien état s'efface ; au « décollage », la classe du nouvel état se pose, son effet surgit et les lettres du mot se posent en rebondissant. Un geste propre à chaque état d'arrivée (sursaut, penché, grand saut, double saut, recul, soupir…, [`motion.ts`](src/mascotte/motion.ts)). Un geste qui en interrompt un autre part de la pose courante : jamais de saut (test `aucune image ne saute`).
* **Vie** : coups d'œil, clignements, petits gestes espacés par état ; en veille, animations rares et aléatoires (un œil qui s'entrouvre, tête qui tombe, bâillement, frisson, lettre brouillée, tour de la pièce du regard), jamais deux fois la même d'affilée.
* **Curseur** : le regard et le corps suivent le curseur sur la fenêtre ; le survol réveille Kobi en veille ; survoler la pilule l'écrase comme un bonbon.
* **Données** : la bouche suit la réponse en flux (voyelles, ponctuation) et se tait quand le flux s'arrête ; une source trouvée fait « ! » ; une note prise ou une phrase finie, un hochement.
* **Écoute** : Kobi suit le volume du micro (événement `niveau` de la coque, de 0 à 1, ~20 Hz) ; sans valeur, il retombe seul.
* **Réunion** : événement `enregistrement` de la coque (`{ actif, pause, debut_ms, notes, titre }`, et commande `etat_enregistrement` au démarrage) ; repli sur l'état de session. Anneau rouge qui pulse autour de la pilule dans tous les états, étiquette « REC · titre · durée · notes » collée à la pilule, rebond du compteur et « +1 » à chaque note ([`src/recording.ts`](src/recording.ts)).
* **Lisibilité** : la fenêtre est transparente ; le texte noir (mot, symboles) a un contour vert peint dessous, la pilule un liseré vert : lisible sur un bureau sombre, clair ou une photo.
* **Sobriété** : hors transitions et gestes, seules les boucles CSS de l'état tournent ; la vie est faite de minuteries espacées. Fenêtre masquée ou repliée : aucune minuterie, les gestes en cours se terminent, les boucles sont figées. « Réduire les animations » : expressions gardées, aucun mouvement, ni gestes ni curseur.

## Polices

Recursive (SIL Open Font License 1.1, [`src/fonts/OFL-Recursive.txt`](src/fonts/OFL-Recursive.txt)), fichier variable complet (axes `wght`, `CASL`, `slnt`, `MONO`, `CRSV`), jeu latin, issu de `@fontsource-variable/recursive` 5.3.0. Embarquée, jamais chargée depuis Internet.
