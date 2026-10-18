; Installeur Windows de Kobi : crochets NSIS de Tauri (KOB-143, KOB-161).
;
; Après l'installation, propose le démarrage avec Windows (Oui par défaut).
; La valeur écrite est celle que lit et règle l'application
; (`crates/kobi-app/src/demarrage.rs`) : HKCU\…\Run, « Kobi ».
; En installation silencieuse (/S), le démarrage est activé.
; Le choix est noté dans %USERPROFILE%\Kobi\demarrage.choisi : le premier
; lancement de Kobi ne le change plus (sans ce fichier, il coche la case).
;
; La CLI de Kobi (KOB-161) : `kobi-ctl.exe` (binaire console, installé à côté
; de l'app par `tauri.installeur.conf.json`) est copiée en `bin\kobi.exe`, et
; `bin` est ajouté au PATH de l'utilisateur : dans un terminal, `kobi status`,
; `kobi mcp`… L'app graphique `kobi.exe` reste hors du PATH.

; Clé du PATH de l'utilisateur (une autre clé dans l'essai de cette logique).
!ifndef KOBI_ENV
  !define KOBI_ENV "Environment"
!endif
; Recherche d'une sous-chaîne, sans tenir compte de la casse (comme Windows
; pour les chemins) : $KobiPos = position de $KobiAiguille dans $KobiBotte,
; ou -1.
Var KobiBotte
Var KobiAiguille
Var KobiPos

!macro KOBI_FONCTION_CHERCHER prefixe
Function ${prefixe}KobiChercher
  Push $R0
  Push $R1
  StrLen $R0 $KobiAiguille
  StrCpy $KobiPos 0
  kobi_boucle:
    StrCpy $R1 $KobiBotte $R0 $KobiPos
    StrCmp $R1 "" kobi_absent
    StrCmp $R1 $KobiAiguille kobi_fin
    IntOp $KobiPos $KobiPos + 1
    Goto kobi_boucle
  kobi_absent:
    StrCpy $KobiPos -1
  kobi_fin:
  Pop $R1
  Pop $R0
FunctionEnd
!macroend
!insertmacro KOBI_FONCTION_CHERCHER ""
!insertmacro KOBI_FONCTION_CHERCHER "un."

; Prévient les terminaux ouverts ensuite que le PATH a changé.
!macro KOBI_PREVENIR_ENVIRONNEMENT
  ; HWND_BROADCAST, WM_SETTINGCHANGE
  SendMessage 0xFFFF 0x1A 0 "STR:${KOBI_ENV}" /TIMEOUT=5000
!macroend

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
  CreateDirectory "$PROFILE\Kobi"
  FileOpen $0 "$PROFILE\Kobi\demarrage.choisi" w
  FileClose $0
  !insertmacro KOBI_CLI_INSTALLER
!macroend

; CLI : bin\kobi.exe, et bin dans le PATH de l'utilisateur.
!macro KOBI_CLI_INSTALLER
  CreateDirectory "$INSTDIR\bin"
  CopyFiles /SILENT "$INSTDIR\kobi-ctl.exe" "$INSTDIR\bin\kobi.exe"
  ClearErrors
  ReadRegStr $KobiBotte HKCU "${KOBI_ENV}" "Path"
  ; Erreur : pas de PATH utilisateur, ou plus long que ce que NSIS sait lire.
  ; Dans le doute, on n'y touche pas (jamais de PATH tronqué).
  IfErrors kobi_path_illisible
  StrCpy $KobiAiguille "$INSTDIR\bin"
  Call KobiChercher
  IntCmp $KobiPos -1 0 kobi_path_fin kobi_path_fin
  StrCmp $KobiBotte "" 0 +3
    StrCpy $KobiBotte "$INSTDIR\bin"
    Goto +2
    StrCpy $KobiBotte "$KobiBotte;$INSTDIR\bin"
  StrLen $KobiPos $KobiBotte
  IntCmp $KobiPos ${NSIS_MAX_STRLEN} kobi_path_illisible 0 kobi_path_illisible
  WriteRegExpandStr HKCU "${KOBI_ENV}" "Path" $KobiBotte
  !insertmacro KOBI_PREVENIR_ENVIRONNEMENT
  DetailPrint "CLI de Kobi : $INSTDIR\bin ajouté au PATH de l'utilisateur"
  Goto kobi_path_fin
  kobi_path_illisible:
  DetailPrint "CLI de Kobi : PATH non modifié ; la CLI est dans $INSTDIR\bin\kobi.exe"
  kobi_path_fin:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro KOBI_CLI_DESINSTALLER
!macroend

!macro KOBI_CLI_DESINSTALLER
  Delete "$INSTDIR\bin\kobi.exe"
  RMDir "$INSTDIR\bin"
  ClearErrors
  ReadRegStr $KobiBotte HKCU "${KOBI_ENV}" "Path"
  IfErrors kobi_retrait_fin
  StrCmp $KobiBotte "$INSTDIR\bin" 0 +3
    DeleteRegValue HKCU "${KOBI_ENV}" "Path"
    Goto kobi_retrait_prevenir
  StrCpy $KobiAiguille ";$INSTDIR\bin"
  Call un.KobiChercher
  IntCmp $KobiPos -1 kobi_retrait_fin
  ; Avant + après la sous-chaîne retirée.
  Push $R0
  Push $R1
  StrLen $R0 $KobiAiguille
  IntOp $R0 $R0 + $KobiPos
  StrCpy $R1 $KobiBotte "" $R0
  StrCpy $KobiBotte $KobiBotte $KobiPos
  StrCpy $KobiBotte "$KobiBotte$R1"
  Pop $R1
  Pop $R0
  WriteRegExpandStr HKCU "${KOBI_ENV}" "Path" $KobiBotte
  kobi_retrait_prevenir:
  !insertmacro KOBI_PREVENIR_ENVIRONNEMENT
  kobi_retrait_fin:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Kobi"
!macroend
