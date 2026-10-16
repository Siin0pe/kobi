# Mot-clé « Kobi » : jeu d'entraînement et modèle openWakeWord

Clips de synthèse pour entraîner et tester les détecteurs de mot-clé
(openWakeWord, KOB-26 ; rustpotter, KOB-27), et entraînement du modèle
openWakeWord. Aucun modèle « Kobi » n'existe ailleurs.

## Générer le jeu

```sh
python bench/wake/generate.py                 # 1500 positifs, 600 mots proches, 300 phrases par langue
python bench/wake/generate.py --positives 200 # plus petit
python bench/wake/generate.py --out DIR       # ailleurs que ~/Kobi-bench/wake
```

Mêmes prérequis que `bench/corpus/` (Python 3.10+, numpy ; Piper et les voix
sont téléchargés dans `~/Kobi-bench/`). `--threads` (défaut 4) limite le
process et ses enfants à 4 processeurs logiques, en priorité basse.

## Contenu

| Classe | Formes | Exemple |
| -- | -- | -- |
| positif `wake` | le nom seul | « Kobi. » |
| positif `hey` | « Hey / Hé / Dis / Okay Kobi » | « Hey Kobi. » |
| positif `command` | « Kobi, » puis une commande | « Kobi, crée un ticket. » |
| négatif `near` | mots et phrases proches | « copie », « robot », « Toby », « Gobi », « Fais une copie. » |
| négatif `phrase` | phrases de réunion sans « Kobi », tirées de `bench/corpus/dialogues/` | « On passe à la suite. » |

**Variété.** Chaque clip tire sa voix, sa vitesse (`length_scale` 0,8 à 1,3),
sa hauteur (facteur 0,9 à 1,1, qui crée des pseudo-locuteurs) et le bruit du
générateur Piper (`noise_scale`/`noise_w` : 0/0, 0,333/0,333 ou 0,667/0,8).
Avec un bruit nul, une voix dit toujours un mot exactement de la même façon ;
le bruit varie l'intonation et les durées d'un clip à l'autre. Une même
combinaison n'est jamais tirée deux fois. Le bruit de fond et la
réverbération sont ajoutés à l'entraînement (voir plus bas), pas dans le jeu.

* **Voix** : Piper `fr_FR-tom`, `fr_FR-siwis`, `fr_FR-upmc` (pierre),
  `fr_FR-gilles`, `en_US-ryan`, `en_US-amy`, `en_GB-alan`, `en_GB-jenny_dioco`,
  `en_US-libritts_r` (904 locuteurs), `en_GB-vctk` (109), `en_GB-cori`,
  `en_US-kristin`, `en_US-norman`, `en_US-john` ; voix Windows Hortense et Zira
  quand elles sont installées. La licence des données de chaque voix est dans
  `kobitts.VOICE_LICENSES` et dans le manifeste.
* **Prononciation** : en anglais le moteur reçoit « Koby » (/ˈkoʊbi/), sinon il
  dit « Ko-bye ».
* **Homophones** : un négatif dont les phonèmes eSpeak contiennent ceux de
  « Kobi » est écarté et listé dans le manifeste. C'est le cas de « Coby »,
  homophone en anglais et dit à l'anglaise par les voix françaises.
* **Clips anormaux** : un clip dont la durée par caractère dépasse 2,5 fois la
  médiane de sa langue est écarté (traîne des voix « low » sans bruit).
* **`wake_end`** : fin du mot-clé dans le clip, en secondes. Pour « Kobi, … »,
  le nom et la commande sont synthétisés séparément puis assemblés avec
  0,15 s de pause, pour connaître cette fin exactement.

**Reproductibilité.** Les clips sans bruit Piper sont identiques octet pour
octet d'une génération à l'autre ; Piper tire le bruit sans graine, donc les
autres varient légèrement. Le manifeste donne les SHA-256 de la génération.

## Sortie

```
~/Kobi-bench/wake/
  manifest.json                     comptes, voix et licences, homophones, clips écartés, SHA-256
  metadata.jsonl                    une ligne par clip
  positive/{train,test}/*.wav       16 kHz, mono, 16 bits
  negative/{train,test}/*.wav
```

Une ligne de `metadata.jsonl` :

```json
{"file": "positive/train/fr-command-00001.wav", "label": "positive", "kind": "command", "split": "train", "lang": "fr", "text": "Kobi, pause.", "spoken": "Kobi, pause.", "voice": "sapi:Microsoft Hortense Desktop", "length_scale": 1.3, "pitch": 1.05, "noise_scale": 0.0, "noise_w": 0.0, "duration": 1.519, "wake_end": 0.716}
```

`test` ne contient que des voix jamais entendues en `train` : `fr_FR-tom`,
`en_US-ryan`, `en_US-amy` et un locuteur sur dix des voix multi-locuteurs.
`tom` (AGPL) et `ryan` (CC BY-NC-SA) n'entrent ainsi dans aucun modèle.

## Entraîner le modèle openWakeWord

```sh
python -m venv ~/Kobi-bench/venv-wake
~/Kobi-bench/venv-wake/Scripts/pip install numpy onnxruntime onnx onnxscript
~/Kobi-bench/venv-wake/Scripts/pip install torch --index-url https://download.pytorch.org/whl/cu128
~/Kobi-bench/venv-wake/Scripts/python bench/wake/train_oww.py   # --device cpu sans GPU
```

Chaîne openWakeWord : mel (`melspectrogram.onnx`) puis embeddings
(`embedding_model.onnx`), tous deux de la release v0.5.1, téléchargés dans
`~/Kobi-bench/models/openwakeword/` ; seul le petit modèle final (16 vecteurs
de 96 valeurs, soit environ 2 s d'audio → score) est entraîné, sur GPU si
disponible. Les features sont calculées sur CPU (`--threads`, défaut 4, en
priorité basse).

Augmentation, tirée à chaque passage (3 par positif, 2 par négatif) :
position du mot-clé dans la fenêtre, réverbération synthétique (RT60 de 0,15 à
0,8 s) une fois sur deux, fond (bruit blanc, rose, brun, ou brouhaha de 1 à 3
voix tiré des phrases négatives d'entraînement) à un rapport signal sur bruit
de 0 à 25 dB, gain. S'y ajoutent 1 500 clips de fond seul.

Sortie dans `~/Kobi-bench/models/kobi-oww/` : `kobi.onnx` (entrée `x`
[n, 16, 96], sortie `score` [n, 1]), `metrics.json` et `card.json`.

Évaluation, à plusieurs seuils : positifs et négatifs de `test`, propres et
bruités ; les 15 interpellations et les faux déclenchements du corpus
synthétique (`bench/corpus/`) ; les faux déclenchements par heure sur le jeu
de validation d'openWakeWord (10,7 h de features précalculées, téléchargées
depuis `davidscripka/openwakeword_features`, utilisées pour l'évaluation
seulement).

Le même modèle tourne en Rust dans `kobi-wake` (`oww::OpenWakeWord`), sur
CPU : la machine cible n'a pas de GPU (le GPU ne sert qu'à l'entraînement).
`cargo run --release -p kobi-wake --example wake_eval` rejoue la partition
`test` en flux pour comparer avec Python ; `--example wake_cpu` mesure la
charge au repos (audio en temps réel par blocs de 80 ms, 1 thread ONNX par
modèle) : CPU du process en % d'un cœur, calcul par bloc, mémoire.

## Limites

* **Licence : prototype non distribuable.** Presque toutes les voix Piper
  sont affinées depuis `lessac` (licence de recherche) et les voix Windows
  relèvent de la licence Microsoft. Un modèle livrable devra être réentraîné
  sur des voix propres (`cori`, `kristin`, `norman`, `john` en anglais ; aucune
  en français pour l'instant) et sur des enregistrements réels consentis.
* **Peu de voix françaises** : 5 locuteurs réels, dont un en test.
  `fr_FR-mls-medium`, `fr_FR-mls_1840-low` et `upmc#jessica` sont écartées
  (charabia, vérifié avec Whisper). Les 50 « Kobi » réels du propriétaire
  restent indispensables.
* Hortense et Zira n'existent que sous Windows : ailleurs, le jeu est généré
  sans elles.
* Négatifs : environ 1 h de parole synthétique seulement ; openWakeWord
  s'entraîne d'habitude avec des milliers d'heures de négatifs génériques
  (ACAV100M, non utilisé ici pour des raisons de licence et de taille).
