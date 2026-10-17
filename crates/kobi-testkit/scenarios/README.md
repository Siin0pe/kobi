# Scénarios du faux `claude`

Chaque fichier rejoue une session `claude -p --input-format stream-json
--output-format stream-json --verbose --include-partial-messages`. Les lignes
viennent de sessions réelles enregistrées avec le faux serveur MCP `fake-mcp`
(serveur `notes`), raccourcies (liste des outils de `init`, identifiants) et
complétées de directives.

```json
{
  "turns": [
    { "expect": "texte attendu dans le message", "lines": [ … ] }
  ]
}
```

Un tour par message reçu sur stdin, dans l'ordre. `expect` (facultatif) :
le message doit contenir ce texte, sinon le faux `claude` s'arrête en
erreur (code 2) avec le message sur stderr. Un message de plus que de tours
fait de même.

Chaque élément de `lines` est soit une ligne stream-json écrite telle quelle
sur stdout, soit une directive :

| Directive | Effet |
| -- | -- |
| `{"fake":"sleep","ms":N}` | Pause de N ms (délais, « je cherche encore »). |
| `{"fake":"exit","code":N,"stderr":"…"}` | Plantage : message sur stderr, code N. |
| `{"fake":"call","tool":"mcp__notes__read_notes","input":{}}` | Appelle l'outil MCP pour de vrai ; son texte remplace `{{tool_result}}` dans les lignes suivantes. |
| `{"fake":"permission","tool":"…","input":{…},"tool_use_id":"…","allow":[…],"deny":[…]}` | Appelle l'outil de `--permission-prompt-tool` (serveur « kobi ») ; si autorisé, appelle l'outil puis rejoue `allow`, sinon rejoue `deny` avec le message de refus dans `{{deny_message}}`. |

Variables substituées dans toutes les lignes : `{{pid}}` (process du faux
`claude`), `{{turn}}` (numéro du tour, dès 1), `{{tool_result}}` et
`{{deny_message}}` (voir ci-dessus).

Les serveurs MCP viennent de `--mcp-config` (JSON ou chemin de fichier),
en stdio (`command`, `args`) ou en HTTP (`url`).
