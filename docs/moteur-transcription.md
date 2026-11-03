# Ajouter un moteur de transcription

Kobi transcrit en local et en flux : chaque moteur reçoit des trames audio et
renvoie des morceaux de texte au fil de la parole. Tous les moteurs passent
par la même interface, le trait `SttEngine` de la crate `kobi-stt`. On les
compare avec `kobi-bench` avant de les brancher dans l'app.

L'app utilise aujourd'hui Nemotron 3.5 ASR Streaming (par sherpa-onnx, sur le
processeur). Whisper, Kyutai et Voxtral sont des adaptateurs de banc d'essai.

## 1. Le contrat : `SttEngine`

Défini dans `crates/kobi-stt/src/lib.rs`.

- Entrée : des `AudioFrame` en **16 kHz mono**, échantillons `f32` dans
  [-1, 1], avec leur `source` (`Moi` pour le micro, `Eux` pour l'audio de
  l'ordinateur) et leur position `t` en secondes. Les trames d'une même
  source arrivent dans l'ordre ; deux sources peuvent s'entrelacer.
- Sortie : des `Segment` (`start`, `end`, `source`, `text`, `is_final`). Un
  segment **partiel** peut être remplacé par le suivant ; un segment
  **final** ne change plus.

Deux méthodes à écrire :

```rust
impl SttEngine for MonMoteur {
    fn name(&self) -> &str { "mon-moteur" }

    /// Reçoit une trame, rend les segments produits depuis l'appel précédent.
    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>> { … }

    /// Fin du flux : les derniers segments, tous finaux.
    fn finish(&mut self) -> Result<Vec<Segment>> { … }
}
```

Les autres méthodes ont un comportement par défaut. Il suffit de les écrire
si le moteur en est capable :

- `nouvel_enonce(source)` : une parole commence après un silence ; le moteur
  peut repartir d'un état neuf.
- `conclure(source)` : l'écoute a conclu l'énoncé en cours ; ne pas redonner
  ce texte.
- `transcrire_court(samples)` : transcrire à part un mot très court (« oui »,
  « non ») que le flux n'a pas rendu. C'est ce qui permet de répondre à une
  carte de validation à la voix.
- `transcrire_extrait(samples)` : transcrire un extrait à part, sans toucher
  au flux (écoute anticipée quand la transcription prend du retard).
- `build_info()` : options de compilation qui comptent pour la vitesse,
  reprises dans les rapports.

`crates/kobi-stt/src/fake.rs` (`FakeEngine`) est un exemple complet et court.

## 2. Le module dans `kobi-stt`

1. Créer `crates/kobi-stt/src/mon_moteur.rs`.
2. Le déclarer derrière une **feature** dans `crates/kobi-stt/Cargo.toml`, avec
   ses dépendances en `optional = true`, comme `nemotron`, `whisper`, `kyutai`
   ou `voxtral` : un moteur lourd ne doit pas alourdir les autres builds.
3. Dans `lib.rs` : `#[cfg(feature = "mon-moteur")] pub mod mon_moteur;`.
4. Des tests sur de courts WAV (`wav.rs` lit les fichiers,
   `replay::WavReplayer` les rejoue au rythme réel ou accéléré).

## 3. Le mesurer avec `kobi-bench`

`crates/kobi-bench/src/engines.rs` liste les moteurs compilés (`native()`) et
les construit par leur nom (`build()`). Il faut y ajouter le nom du moteur,
derrière la même feature, et sa construction à partir d'`EngineOptions`
(dossier des modèles, appareil `cpu` ou `cuda`). La feature se relaie dans
`crates/kobi-bench/Cargo.toml` : `mon-moteur = ["kobi-stt/mon-moteur"]`.

Le banc rejoue un corpus de dialogues (`bench/corpus`) dans chaque moteur et
écrit `rapport.json` et `rapport.md` : taux d'erreur par mot, latence des
partiels et des finaux, facteur temps réel. Un moteur qui ne suit pas le
temps réel est arrêté (`SttError::TooSlow`). C'est la condition minimale,
puisque Kobi transcrit en direct, et souvent deux pistes à la fois en réunion.

```sh
cargo run --release -p kobi-bench --features mon-moteur -- --help
```

## 4. Le brancher dans l'app

L'app charge Nemotron dans `crates/kobi-app/src/interpellation.rs`
(`charger_stt`). Le moteur y est utilisé par l'écoute du micro
(`ecoute.rs`), la piste de l'ordinateur en réunion (`son_ordi.rs`) et
l'interpellation. Pour proposer un autre moteur :

- rendre ces trois endroits génériques sur `SttEngine` (ou utiliser
  `Box<dyn SttEngine>`), au lieu de `NemotronEngine` ;
- choisir le moteur selon le réglage `stt.engine` de `config.toml`, déjà
  déclaré mais pas encore lu par l'app ;
- ajouter son modèle au téléchargement du premier lancement
  (`crates/kobi-app/src/bienvenue.rs`) et au panneau Réglages › Transcription.

Le moteur doit rester **local** : l'audio ne quitte jamais l'ordinateur
(voir la section Confidentialité du README).
