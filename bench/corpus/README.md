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
de dossier) et vérifie leurs SHA-256. Rien n'est écrit dans le dépôt.
`--threads` (défaut 4) limite le process et ses enfants à 4 processeurs
logiques, en priorité basse. Piper est
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
* Les voix anglaises lisent « Kobi » « Ko-bye » : le moteur reçoit « Koby »
  en anglais (`PRONUNCIATION` dans `bench/tts/kobitts.py`), la référence
  garde « Kobi ».
* Dans `mixte`, un même interlocuteur change de voix quand il change de
  langue : les voix Piper sont monolingues.

# Corpus réel français (SUMM-RE)

Pour mesurer le français de vraies réunions (KOB-212), `summre.py` extrait
cinq passages d'environ 5 minutes du split `test` de
[SUMM-RE](https://huggingface.co/datasets/linagora/SUMM-RE) : des réunions de
3 ou 4 personnes en français spontané (hésitations, chevauchements, rires),
enregistrées sur Zoom, à domicile, en studio et en laboratoire, avec un micro
par participant, transcrites et alignées à la main.

Licence : **CC BY-SA 4.0** ; crédit : SUMM-RE, LINAGORA et Laboratoire Parole
et Langage (Aix-Marseille Université), projet ANR-20-CE23-0017. Le corpus
n'est pas versionné : le script le reconstruit à l'identique depuis une
révision épinglée du jeu.

```sh
pip install pyarrow numpy soundfile soxr fsspec aiohttp requests
python bench/corpus/summre.py            # vers ~/Kobi-bench/corpus-reel-fr
python bench/corpus/summre.py --out DIR --cache DIR
```

Le premier lancement télécharge environ 4 Go de fichiers parquet (pistes de
48 kHz) et garde les pistes ré-échantillonnées à 16 kHz dans `--cache`.

Même format que le corpus synthétique (`moi.wav`, `eux.wav`,
`reference.jsonl`, `reference.txt`, `manifest.json`), avec en plus :

* `moi` est le participant qui parle le plus dans l'extrait, `eux` la somme
  des autres pistes (le son de l'ordinateur en visio) ;
* chaque ligne de `reference.jsonl` porte `locuteur`, `text_brut` (texte
  SUMM-RE d'origine) et `words` (mots horodatés à la main) ;
* dans `text`, rires (`@`), inaudibles (`*`), mots coupés (`addi-`) et
  hésitations (`euh`, `mh`…) sont retirés, `parce_que` devient `parce que` ;
* les pistes restent brutes, bruit de fond compris : la voix des voisins
  arrive dans chaque micro au niveau du bruit de fond, 20 à 30 dB sous la
  parole propre (`--attenuation N` atténuerait chaque micro hors de sa
  parole annotée).

Limite : aucun passage n'interpelle « Kobi » ; le repérage du nom se mesure
sur le corpus synthétique et les corpus du mot-clé.
