# Kobi

Assistant vocal de bureau, local et open source : on l'appelle par son nom, et en réunion il transcrit en local, prend des notes, répond quand on l'interpelle et agit via vos MCP après un clic de confirmation. Fonctionne avec Claude Code.

Projet en cours de construction, pas encore utilisable.

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
