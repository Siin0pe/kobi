# Kobi

Assistant vocal de bureau, local et open source : on l'appelle par son nom, et en réunion il transcrit en local, prend des notes, répond quand on l'interpelle et agit via vos MCP après un clic de confirmation. Fonctionne avec Claude Code, ou sans lui avec une clé de l'API Anthropic.

![Kobi en réunion : il prend des notes, on l'interpelle, il cherche dans le code et Linear, répond, puis propose un ticket créé d'un clic](docs/demo/kobi-demo.gif)

Projet en cours de construction : pas encore de version publiée.

## Ce que fait Kobi

- **On lui parle.** « Kobi, … » au micro, ou un raccourci clavier (Ctrl+Maj+Espace pour parler, Ctrl+Alt+K pour écrire). Il répond dans une bulle, à voix haute s'il est réglé ainsi, et « stop » le fait taire.
- **En réunion** (Ctrl+Maj+R, ou « Kobi, enregistre la réunion »), il transcrit votre micro et le son de l'ordinateur (les autres participants en visio), prend des notes par blocs (décisions, actions, questions) et écrit le compte rendu à la fin.
- **Il cherche pour vous**, dans vos dossiers de code (en lecture seule) et vos outils MCP (Linear, Notion…), et montre chaque étape de son travail.
- **Il n'agit qu'après confirmation.** Écrire un fichier, lancer une commande ou créer un ticket passe par une carte « Valider / Refuser », sauf ce que vous avez autorisé dans les permissions.
- **Le Carnet** garde vos réunions, conversations et rappels en fichiers Markdown lisibles, consultables et cherchables dans l'app.
- **En français ou en anglais**, au choix ou selon la langue du système, sans relancer.

## Confidentialité

- **L'audio ne quitte jamais votre ordinateur.** La transcription (Nemotron, par sherpa-onnx) et la voix de Kobi (Piper) tournent en local, sur le processeur. Aucun son n'est enregistré : hors réunion, Kobi écoute seulement pour entendre son nom, et seul le texte de vos échanges avec lui est gardé dans le Carnet.
- **Ce qui part vers Claude**, par votre propre Claude Code ou par l'API d'Anthropic avec votre clé : le texte de vos questions, les extraits de transcription utiles aux notes et au compte rendu, et ce que Claude lit dans les dossiers et outils que vous lui avez ouverts. Les conditions d'Anthropic s'appliquent à ces échanges, comme à tout usage de Claude Code ou de l'API.
- **Votre clé API**, si vous en utilisez une, est gardée dans le gestionnaire d'identifiants du système, jamais dans `config.toml` ni dans les journaux.
- **Vos MCP** reçoivent seulement ce que Kobi leur demande, après votre confirmation pour toute écriture. Leurs secrets sont gardés dans le trousseau du système.
- **Tout est stocké chez vous**, dans `~/Kobi` par défaut (dossier réglable) : réunions, conversations, rappels et `config.toml`, avec une durée de conservation réglable. Pas de compte, pas de télémétrie.
- **Seuls téléchargements** : les modèles de transcription et de voix, au premier lancement, depuis les publications de sherpa-onnx sur GitHub.
- **En réunion**, Kobi propose un message à coller dans le chat pour prévenir les participants.

Kobi est un projet indépendant : il utilise Claude Code, mais il n'est ni affilié à Anthropic ni approuvé par Anthropic.

## Installer depuis les sources

Il faut :

- [Rust](https://rustup.rs) (la version est fixée par `rust-toolchain.toml`) et [Node.js](https://nodejs.org) 20 ou plus ;
- les dépendances système de Tauri 2 ([prérequis](https://v2.tauri.app/start/prerequisites/) : WebView2 sous Windows, `webkit2gtk-4.1` et ALSA/PulseAudio sous Linux) ;
- [Claude Code](https://docs.anthropic.com/en/docs/claude-code), installé et connecté (`claude auth login`), **ou** une clé de l'API Anthropic, à saisir dans Réglages › Clé API.

Le cerveau se choisit avec `brain` dans `config.toml` : `"auto"` (par défaut) prend Claude Code s'il est installé, sinon la clé API ; `"claude-code"` ou `"api"` l'imposent. Avec la clé API, Kobi lit lui-même vos dossiers de code (en lecture seule, sans jamais sortir des dossiers déclarés) et appelle vos MCP.

```sh
npm --prefix ui ci
npm --prefix ui run build
cargo build --release -p kobi-app --features custom-protocol
./target/release/kobi
```

Au premier lancement, Kobi vous guide en dix étapes : langue, Claude Code, micro, son de l'ordinateur, modèle de transcription (téléchargé, environ 450 Mo), dossiers de code, outils MCP, démarrage avec le système, un essai « Kobi, bonjour » et les raccourcis. Tout se retrouve ensuite dans les Réglages.

Pour développer : `npm --prefix ui run dev` dans un terminal, `cargo run -p kobi-app` dans un autre (l'interface se recharge à chaud). Les tests : `cargo test --workspace` et `npm --prefix ui test` (Playwright). La CI (GitHub Actions) lance les tests Rust sous Linux, Windows et macOS, et ceux de l'interface sous Linux. Voir aussi [CONTRIBUTING.md](CONTRIBUTING.md).

## Guides

- [Ajouter un moteur de transcription](docs/moteur-transcription.md)
- [Stockage et conservation](docs/stockage.md)
- [Démarrage avec le système](docs/demarrage.md)
- Régénérer le GIF de démonstration : `npm --prefix ui run demo-gif` (Chromium de Playwright et ffmpeg ; la scène est dans `ui/tests/demo-gif.spec.ts`)

## Piloter Kobi : CLI et MCP

Kobi se pilote aussi par programme, depuis le terminal ou depuis un autre agent. Les commandes parlent à l'app Kobi en cours, par un canal local (canal nommé sous Windows, socket Unix ailleurs) protégé par un jeton, jamais exposé sur le réseau.

```sh
kobi status                          # état de Kobi
kobi app open --section notes        # ouvre Kobi sur ses notes
kobi meeting start                   # démarre l'enregistrement (prise de notes)
kobi meeting stop                    # termine ; le compte rendu s'écrit ensuite
kobi notes last --format md          # notes et compte rendu de la dernière réunion
kobi meetings export last --out cr.md
kobi search budget
kobi config set voice.assistant true # vérifié, écrit dans config.toml
kobi config undo
kobi reminders add "Appeler Léa" --due 2026-10-19T09:00
kobi help                            # toutes les commandes
```

`--json` donne le résultat brut. Codes de retour : 0 succès, 1 commande refusée, 2 usage, 3 Kobi ne tourne pas, 4 jeton refusé.

**Installation de la commande `kobi`.** Sous Windows, l'installeur place la CLI dans `bin\kobi.exe`, à côté de l'app (par défaut `%LOCALAPPDATA%\Kobi\bin`), et ajoute ce dossier au `PATH` de l'utilisateur : ouvrez un nouveau terminal et tapez `kobi status`. La désinstallation retire le dossier du `PATH`. Si votre `PATH` est trop long pour que l'installeur le modifie sans risque, il n'y touche pas : ajoutez le dossier vous-même ou utilisez le chemin complet. L'app graphique `Kobi.exe` comprend aussi ces commandes, mais un terminal Windows n'attend pas sa fin : préférez `kobi` (ou `kobi-ctl.exe`), qui rend proprement son code de retour.

Construire l'installeur Windows avec la CLI :

```powershell
powershell -ExecutionPolicy Bypass -File crates\kobi-app\windows\installeur.ps1
```

### Brancher Kobi dans Claude Code (ou un autre agent)

```sh
claude mcp add kobi -- kobi mcp
```

Avec le chemin complet si `kobi` n'est pas dans le `PATH`, par exemple `claude mcp add kobi -- "%LOCALAPPDATA%\Kobi\bin\kobi.exe" mcp`. Les outils (`status`, `notes_last`, `meetings_get`, `meeting_start`, `config_set`…) annoncent chacun s'ils lisent (`readOnlyHint`) ou modifient. Kobi branche ce même MCP dans ses propres sessions, en lecture seule tant que la validation à la voix ou par carte n'est pas en place.

## Ajouter une langue

L'interface de Kobi se traduit à chaud. La langue suit celle du système, ou le réglage `ui.language` (Réglages › Langue, ou `kobi config set ui.language en`).

1. Copier `ui/src/i18n/fr.json` en `ui/src/i18n/xx.json` (`xx` : le code de la langue, par exemple `de`) et traduire chaque valeur. Les clés restent les mêmes, et les variables `{nom}` gardent leur nom. Un pluriel s'écrit `{ "one": "…", "other": "…" }`, avec les catégories de `Intl.PluralRules` de la langue.
2. Ajouter la langue à `LANGUES` dans `ui/src/i18n/index.ts` (son nom écrit dans sa propre langue, par exemple « Deutsch »).
3. Côté Rust : ajouter la variante à `Langue`, le fichier à `source()` et la règle de pluriel à `pluriel()` (`crates/kobi-i18n/src/lib.rs`). Les textes de la coque, de la CLI et des permissions passent par `kobi_i18n::tr("clé", …)` ; `cargo test -p kobi-i18n` vérifie que chaque clé employée existe dans chaque langue.
4. `cargo test -p kobi-i18n` vérifie que chaque langue a toutes les clés, avec les mêmes variables.
