; Installeur Windows de Kobi : crochets NSIS de Tauri (KOB-143).
;
; Après l'installation, propose le démarrage avec Windows (Oui par défaut).
; La valeur écrite est celle que lit et règle l'application
; (`crates/kobi-app/src/demarrage.rs`) : HKCU\…\Run, « Kobi ».
; En installation silencieuse (/S), le démarrage est activé.

!macro NSIS_HOOK_POSTINSTALL
  IfSilent kobi_demarrage_oui
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON1 \
    "Lancer Kobi au démarrage de Windows ?$\r$\n$\r$\nKobi s'ouvrira en veille, sans fenêtre. Ce choix se change ensuite dans les réglages de Kobi." \
    /SD IDYES IDYES kobi_demarrage_oui
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Kobi"
  Goto kobi_demarrage_fin
  kobi_demarrage_oui:
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Kobi" '"$INSTDIR\${MAINBINARYNAME}.exe" --demarrage'
  kobi_demarrage_fin:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Kobi"
!macroend
