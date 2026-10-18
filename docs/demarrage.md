# Démarrage avec le système (KOB-143)

Kobi peut se lancer tout seul à l'ouverture de session. Il démarre alors en
veille, sans fenêtre de bienvenue : la ligne de commande enregistrée porte
l'argument `--demarrage`.

## Où vit le réglage

Le réglage n'est pas dans `config.toml` : il est là où le système le lit, ce
qui évite deux vérités différentes.

| Système | Emplacement |
| -- | -- |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, valeur `Kobi` = `"…\kobi.exe" --demarrage` |
| Linux | `$XDG_CONFIG_HOME/autostart/kobi.desktop` (défaut `~/.config/autostart`) |
| macOS | `~/Library/LaunchAgents/dev.kobi.app.plist` |

Sous Linux, une entrée désactivée par le bureau (`Hidden=true` ou
`X-GNOME-Autostart-enabled=false`) compte comme absente.

## Installeur Windows

`crates/kobi-app/windows/installeur.nsh` (crochets NSIS de Tauri) demande,
à la fin de l'installation, « Lancer Kobi au démarrage de Windows ? », Oui
par défaut, et écrit ou retire la même valeur `Run`. En installation
silencieuse (`/S`), le démarrage est activé. La désinstallation retire la
valeur.

## Interface

Deux commandes Tauri, pour les réglages et le premier lancement :

* `demarrage_systeme` → `bool` : état actuel, relu dans le système ;
* `regler_demarrage_systeme { actif: bool }` → `bool` : applique, puis relit.
