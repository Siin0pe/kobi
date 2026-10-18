# candle-core 0.2.2, copie patchée pour Kobi

Source : crate `candle-core` 0.2.2 publiée sur crates.io (Hugging Face,
MIT OR Apache-2.0), sans `examples/`, `tests/` ni `Cargo.lock`. Branchée par
`[patch.crates-io]` dans le `Cargo.toml` du workspace.

## Pourquoi

rustpotter 3.0.2 (fonction `rustpotter` de `kobi-wake`, KOB-27) dépend de
candle-core 0.2.2. Le reste du workspace utilise candle 0.9 (Kyutai, KOB-19),
qui exige `half` ≥ 2.5. Or `half` 2.5+ implémente ses distributions pour
`rand_distr` 0.5, alors que candle-core 0.2.2 tire des `bf16`/`f16` avec
`rand` 0.8 et `rand_distr` 0.4 : la crate d'origine ne compile pas, et un
lockfile ne peut contenir qu'un seul `half` 2.x.

## Ce qui change

Uniquement `src/cpu_backend.rs`, fonctions `rand_uniform` et `rand_normal` :
pour `BF16` et `F16`, le tirage se fait en `f32` puis est converti
(`bf16::from_f32`, `f16::from_f32`) au lieu de passer par les impl
`rand_distr` de `half`. Les chemins `F32` et `F64`, les seuls qu'utilise
rustpotter, sont inchangés.

Dans `Cargo.toml` : la fonction `rand_distr` de `half` n'est plus demandée, et
les avertissements de ce code tiers sont coupés (`[lints]`).

Le patch ne remplace que la 0.2.2 : candle 0.9 vient toujours de crates.io.

## Quand le retirer

Dès que rustpotter n'est plus utilisé (choix du détecteur, KOB-28) ou qu'une
version de rustpotter passe à un candle récent : supprimer ce dossier, la
section `[patch.crates-io]` et l'entrée `exclude` du `Cargo.toml` racine.
