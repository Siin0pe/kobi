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

`--json` donne le résultat brut. Codes de retour : 0 succès, 1 commande refusée, 2 usage, 3 Kobi ne tourne pas, 4 jeton refusé. Sous Windows, `kobi.exe` est l'app graphique : dans un terminal, `kobi-ctl.exe` fait la même chose et rend proprement son code de retour.

### Brancher Kobi dans Claude Code (ou un autre agent)

```sh
claude mcp add kobi -- kobi mcp
```

Avec le chemin complet si `kobi` n'est pas dans le `PATH`, par exemple `claude mcp add kobi -- "C:\Program Files\Kobi\kobi.exe" mcp`. Les outils (`status`, `notes_last`, `meetings_get`, `meeting_start`, `config_set`…) annoncent chacun s'ils lisent (`readOnlyHint`) ou modifient. Kobi branche ce même MCP dans ses propres sessions, en lecture seule tant que la validation à la voix ou par carte n'est pas en place.
