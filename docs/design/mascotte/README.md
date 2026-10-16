# Prototypes animés de la mascotte

Prototypes de design (15/10/2026) qui servent de référence pour les skins de Kobi. Ce sont des pages « Design Component » écrites pour l'outil de maquette : le HTML et le CSS (keyframes, transitions) sont lisibles tels quels, mais la logique (`class Component extends DCLogic`, `{{ … }}`, `<sc-if>`, `<sc-for>`) dépend du moteur de l'outil et doit être réécrite pour l'application.

| Fichier | Skin | Idée |
| -- | -- | -- |
| `Glyphe.dc.html` | **Kobi** (par défaut) | Visage fait de lettres en police variable Recursive, vert acide, 8 états, brouillage des caractères aux transitions |
| `Gribouille.dc.html` | Gribouille | Bonhomme à la craie, trait qui tremble (filtre de turbulence SVG), animation saccadée |
| `Mercure.dc.html` | Mercure | Goutte de chrome liquide, accent bleu électrique |
| `Matrice.dc.html` | Matrice | Écran à points LED orange, expressions pixel par pixel |
| `Gelee.dc.html` | Gelée | Verre dépoli avec des lumières colorées internes |
| `Essaim.dc.html` | Essaim | Nuée de 46 particules qui se réorganisent |

Les états, le contrat d'un skin et les règles (réduire les animations, CPU au repos) sont décrits dans le document Linear « Kobi — mascotte et skins ».
