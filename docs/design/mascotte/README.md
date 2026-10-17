# Prototype animé de la mascotte

Prototype de design (15/10/2026) qui sert de référence pour Kobi. C'est une page « Design Component » écrite pour l'outil de maquette : le HTML et le CSS (keyframes, transitions) sont lisibles tels quels, mais la logique (`class Component extends DCLogic`, `{{ … }}`, `<sc-if>`, `<sc-for>`) dépend du moteur de l'outil et a été réécrite pour l'application (`ui/src/mascotte/`).

| Fichier | Idée |
| -- | -- |
| `Glyphe.dc.html` | Visage fait de lettres en police variable Recursive, vert acide, 8 états, brouillage des caractères aux transitions |

Kobi (style Glyphe) est l'identité de la marque et la seule apparence de la mascotte (décision du 18/10/2026, KOB-136). Les états et les règles (réduire les animations, CPU au repos) sont décrits dans le document Linear « Kobi — mascotte et skins ».
