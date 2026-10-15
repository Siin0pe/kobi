# Contribuer à Kobi

Le plan de travail vit dans Linear (team `Kobi`, clé `KOB`). Chaque changement correspond à un ticket.

## Branches

Format standard : `<type>/kob-NN-titre-court`.

| Type | Usage |
| -- | -- |
| `feat` | Nouvelle fonctionnalité |
| `fix` | Correction de bug |
| `chore` | Outillage, dépendances, maintenance |
| `docs` | Documentation |
| `ci` | Intégration continue |
| `test` | Tests seuls |
| `refactor` | Restructuration sans changement de comportement |
| `perf` | Performance |

Exemple : `feat/kob-10-enregistreur-double-piste`.

## Commits et pull requests

* Commits préfixés par le ticket : `KOB-10: capture du loopback WASAPI`.
* Une PR par ticket, vers `main`, fusionnée en squash.
* La CI doit être verte sur Windows, macOS et Linux (`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`).
* Pas de ligne `Co-Authored-By` ni de mention d'outil de génération dans les commits et les PR.

## Code

* Rust stable épinglé dans `rust-toolchain.toml`.
* Tests d'abord pour la session, les permissions et l'envoi incrémental de la transcription.
* Aucun fichier audio, modèle ou secret dans le dépôt.
