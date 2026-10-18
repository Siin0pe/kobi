# Stockage local de Kobi

Tout ce que Kobi garde est écrit en local, en markdown et en JSON lisibles :
c'est la source de vérité, lue par le Carnet et, plus tard, publiée par MCP.
Code : `crates/kobi-session/src/stockage.rs` ; état partagé et réglage à chaud :
`crates/kobi-app/src/stockage.rs`.

## Arborescence

```text
<racine>/                                  défaut ~/Kobi, réglage [stockage] dossier
  reunions/AAAA-MM-JJ-titre/
    reunion.json        {"version":1,"titre":…,"debut_ms":…,"fin_ms":…,"statut":"en-cours"|"terminee","publication":null}
    transcript.jsonl    un segment final par ligne (kobi_stt::Segment), écrit au fil de l'eau
    notes.md            notes prises pendant la réunion, réécrites à chaque bloc
    compte-rendu.md     ## Résumé, ## Décisions, ## Actions, ## Questions ouvertes (+ autres sections)
  conversations/AAAA-MM-JJ/
    HHMMSS.jsonl        une conversation du quotidien, un tour par ligne :
                        {"at_ms":…,"qui":"moi"|"kobi","texte":…,"voix":true|false}
  rappels.json          [{"id","texte","echeance_ms","cree_ms","fait","source"}]
```

`config.toml`, `models/` et `logs/` restent dans `~/Kobi`, quel que soit le
dossier racine. L'ancien dossier `meetings/` est renommé `reunions/` au premier
lancement. Dates et heures des noms de dossier : heure locale.

## Règles

* Transcription et tours de conversation : une ligne par appel, poussée sur le
  disque ; un arrêt brutal ne perd que la ligne en cours, ignorée à la relecture.
* Les autres fichiers sont écrits dans un `.tmp` puis renommés.
* Une conversation n'existe sur le disque qu'à partir de son premier tour.
* Conservation : `[meetings] retention_days` et `[conversations]
  retention_days`, illimitée par défaut pour les deux (« on conserve »),
  réglable parmi 30, 90, 365 ou `"unlimited"` ; purge au démarrage, d'après la date du nom de dossier. La
  réunion en cours n'est jamais purgée.
* Changer de dossier vaut tout de suite pour la réunion ou la conversation
  suivante ; celle en cours finit là où elle a commencé.

## Publication par MCP

```toml
[stockage.publication.reunions]
mcp = "linear"            # nom du serveur MCP ; vide = aucune publication
destination = "Projet Kobi"
```

Le réglage est lu et gardé, et `Stockage::document_reunion` /
`document_conversation` produisent le document à publier (titre + markdown).
Rien n'est envoyé tant que Kobi est en lecture seule (KOB-138) : l'état exposé
est `lecture-seule`.
