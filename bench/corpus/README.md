# Corpus synthétique

Trois réunions d'environ 5 minutes, synthétisées en local, pour construire et
tester le harnais de benchmark avant d'avoir les vraies réunions (KOB-12).

| Dialogue | Langues | Pistes |
| -- | -- | -- |
| `fr` | français | `moi` : `fr_FR-tom-medium`, `eux` : `fr_FR-siwis-medium` |
| `en` | anglais | `moi` : `en_US-ryan-medium`, `eux` : `en_US-amy-medium` |
| `mixte` | `moi` surtout en français, `eux` surtout en anglais | voix choisie par langue de chaque tour |

Chaque dialogue contient des interpellations de « Kobi » des deux côtés, en
français et en anglais, suivies de 3 s de silence (le temps où Kobi répond
dans sa bulle).

## Générer

```sh
python bench/corpus/generate.py            # les trois dialogues
python bench/corpus/generate.py fr         # un seul
python bench/corpus/generate.py --out DIR  # ailleurs que ~/Kobi-bench/corpus
```

Prérequis : Python 3.10+ et numpy. Au premier lancement, le script télécharge
Piper et les voix dans `~/Kobi-bench/` (variable `KOBI_BENCH_HOME` pour changer
de dossier) et vérifie leurs SHA-256. Rien n'est écrit dans le dépôt. Piper est
épinglé pour Windows ; ailleurs, installer le binaire Piper `2023.11.14-2` dans
`~/Kobi-bench/tools/piper/`.

La synthèse est déterministe (bruit du générateur à 0) : deux générations
donnent des fichiers identiques octet pour octet, et `manifest.json` liste
leurs SHA-256.

## Sortie

```
~/Kobi-bench/corpus/
  manifest.json            voix, versions, durées, SHA-256
  <dialogue>/
    moi.wav  eux.wav       16 kHz, mono, 16 bits, même durée
    reference.jsonl        une phrase par ligne
    reference.txt          la même chose, lisible
```

Une ligne de `reference.jsonl` :

```json
{"start": 1.0, "end": 3.392, "source": "moi", "lang": "fr", "kobi": false, "text": "Bonjour Claire, merci d'être là."}
```

`start` et `end` sont en secondes et bornent exactement l'audio de la phrase
(à 30 ms près de marge). `kobi` vaut `true` pour les phrases adressées à Kobi.

## Format des dialogues

`dialogues/<nom>.txt`, une ligne par tour de parole :

```
# commentaire
@lang fr                       langue par défaut
moi: Texte du tour.            piste moi (micro)
eux: Texte du tour.            piste eux (audio système)
eux [en]: Text of the turn.    tour dans une autre langue
moi@kobi: Kobi, question ?     tour adressé à Kobi
```

Les tours sont découpés en phrases sur `.`, `?` et `!` ; chaque phrase est
synthétisée et horodatée séparément.

## Limites

* Voix de synthèse : pas de chevauchement de parole, pas de bruit de fond, pas
  d'hésitations. Le WER mesuré ici sera plus bas que sur de vraies réunions.
* Dans `mixte`, un même interlocuteur change de voix quand il change de
  langue : les voix Piper sont monolingues.
