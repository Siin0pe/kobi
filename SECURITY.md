# Sécurité

Kobi entend vos réunions, garde des transcriptions sur votre disque et peut agir via vos serveurs MCP après confirmation. Les failles qui touchent ces points sont prises au sérieux.

## Signaler une faille

**N'ouvrez pas d'issue publique.** Utilisez le signalement privé de GitHub : onglet **Security** du dépôt, puis **Report a vulnerability**.

Indiquez si possible :

* la version de Kobi et le système (Windows, macOS, Linux) ;
* les étapes pour reproduire, et ce que vous attendiez ;
* l'impact selon vous (lecture de données, action exécutée sans confirmation…).

N'envoyez ni audio ni transcription de vraie réunion : décrivez ou fabriquez un exemple.

Nous accusons réception sous 7 jours et vous tenons informé jusqu'au correctif. Votre nom figure dans les notes de version si vous le souhaitez.

## Dans le périmètre

* Une action MCP exécutée sans la carte de confirmation, ou autre chose que ce qu'elle affichait.
* Une fuite de l'audio, des transcriptions, des notes ou de la mémoire locale hors de la machine, en dehors des envois à Claude que vous avez configurés.
* Une clé API ou un jeton exposé (journaux, fichiers, interface).
* L'exécution de code via un fichier de réunion ou une configuration.

## Hors périmètre

* Les failles de Claude Code, de l'API Anthropic ou d'un serveur MCP tiers : signalez-les à leurs éditeurs.
* Ce qu'un utilisateur confirme lui-même sur la carte de confirmation.

## Versions suivies

Seule la dernière version publiée reçoit des correctifs de sécurité.
