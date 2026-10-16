# Jeu d'entraînement du mot-clé « Kobi »

Clips de synthèse pour entraîner et tester les détecteurs de mot-clé
(openWakeWord, KOB-26 ; rustpotter, KOB-27). Aucun modèle « Kobi » n'existe.

## Générer

```sh
python bench/wake/generate.py                 # 1000 positifs, 600 mots proches, 300 phrases par langue
python bench/wake/generate.py --positives 200 # plus petit
python bench/wake/generate.py --out DIR       # ailleurs que ~/Kobi-bench/wake
```

Mêmes prérequis que `bench/corpus/` (Python 3.10+, numpy ; Piper et les voix
sont téléchargés dans `~/Kobi-bench/`). `--threads` (défaut 3) limite le
process et ses enfants à 3 processeurs logiques, en priorité basse.

La génération est déterministe : deux exécutions donnent les mêmes SHA-256
(`manifest.json`).

## Contenu

| Classe | Formes | Exemple |
| -- | -- | -- |
| positif `wake` | le nom seul | « Kobi. » |
| positif `hey` | « Hey / Hé / Dis / Okay Kobi » | « Hey Kobi. » |
| positif `command` | « Kobi, » puis une commande | « Kobi, crée un ticket. » |
| négatif `near` | mots et phrases proches | « copie », « robot », « Toby », « Gobi », « Fais une copie. » |
| négatif `phrase` | phrases de réunion sans « Kobi », tirées de `bench/corpus/dialogues/` | « On passe à la suite. » |

Chaque clip varie la voix, la vitesse (`length_scale` 0,8 à 1,3) et la hauteur
(facteur 0,9 à 1,1, qui crée des pseudo-locuteurs). Une même combinaison
(locuteur, texte, vitesse, hauteur) n'est jamais tirée deux fois.

* **Voix** : Piper `fr_FR-tom`, `fr_FR-siwis`, `fr_FR-upmc` (pierre),
  `en_US-ryan`, `en_US-amy`, `en_GB-alan`, `en_GB-jenny_dioco`,
  `en_US-libritts_r` (904 locuteurs) ; voix Windows Hortense et Zira quand
  elles sont installées.
* **Prononciation** : en anglais le moteur reçoit « Koby » (/ˈkoʊbi/), sinon il
  dit « Ko-bye ».
* **Homophones** : un négatif dont les phonèmes eSpeak contiennent ceux de
  « Kobi » est écarté et listé dans le manifeste. C'est le cas de « Coby »,
  homophone en anglais et dit à l'anglaise par les voix françaises.
* **`wake_end`** : fin du mot-clé dans le clip, en secondes. Pour « Kobi, … »,
  le nom et la commande sont synthétisés séparément puis assemblés avec
  0,15 s de pause, pour connaître cette fin exactement.

## Sortie

```
~/Kobi-bench/wake/
  manifest.json                     comptes, voix, homophones écartés, SHA-256
  metadata.jsonl                    une ligne par clip
  positive/{train,test}/*.wav       16 kHz, mono, 16 bits
  negative/{train,test}/*.wav
```

Une ligne de `metadata.jsonl` :

```json
{"file": "positive/train/fr-command-00001.wav", "label": "positive", "kind": "command", "split": "train", "lang": "fr", "text": "Kobi, pause.", "spoken": "Kobi, pause.", "voice": "sapi:Microsoft Hortense Desktop", "length_scale": 1.3, "pitch": 1.05, "duration": 1.519, "wake_end": 0.716}
```

`test` contient des voix jamais entendues en `train` : `fr_FR-siwis`,
`en_US-ryan`, `en_US-amy` et un locuteur LibriTTS sur dix.

## Limites

* **Peu de voix françaises** : 4 locuteurs réels (dont un seul en test).
  Écartées après écoute par Whisper : `fr_FR-mls-medium`,
  `fr_FR-mls_1840-low` et `upmc#jessica` (charabia), `fr_FR-gilles-low`
  (traîne d'une seconde après un mot isolé). Les 50 « Kobi » réels du
  propriétaire restent indispensables.
* Hortense et Zira n'existent que sous Windows : ailleurs, le jeu est généré
  sans elles et ses empreintes diffèrent.
* Pas de bruit ni de réverbération : à ajouter à l'entraînement (augmentation
  d'openWakeWord).
