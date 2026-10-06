; Installer hooks (bundle.windows.nsis.installerHooks in tauri.conf.json). The installer includes
; this file before it defines MAINBINARYNAME, MANUKEY and the like, so those appear only inside
; macros, which expand where the installer inserts the hooks.
;
; "Open with". Tauri's file associations make Lectern.Markdown (the association's `name`) the
; default class of each extension, which Windows ignores once the user has chosen a default app for
; it. Windows 11 offers Lectern under "Open with" (where the user can also make it the default)
; only when lectern.exe is registered under Applications with the types it supports; each
; extension also lists the class under OpenWithProgids.
;
; The original default class. Tauri saves an extension's default as `Lectern.Markdown_backup`
; before taking it over and puts it back on uninstall. An update or reinstall takes the extension
; over again and saves our own class as that backup, so a later uninstall would point the extension
; at the class it deletes. So what to restore is kept as `Lectern.Markdown_original`: "none" when
; the extension had no default, else "=" and the class. It is recorded whenever the default isn't
; ours (a first install, or another app took the extension since), and after Tauri's uninstall step
; it is put back, or the default removed, along with everything else the installer added and the
; entry Explorer adds for our class once it has offered it under "Open with".
;
; The open command. Tauri writes the class's command with the exe path unquoted, which an install
; folder with a space in it splits, so it is written again quoted once Tauri is done. Every install
; and update rewrites it, and the uninstaller deletes the class with it.

Var LecternDefault
!define LECTERN_FILEEXTS "Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts"

; Records the extension's default class as what to restore, unless it is ours.
!macro LECTERN_REMEMBER_DEFAULT EXT
  ClearErrors
  ReadRegStr $LecternDefault SHCTX "Software\Classes\.${EXT}" ""
  ${If} ${Errors}
    StrCpy $LecternDefault "none"
  ${Else}
    StrCpy $LecternDefault "=$LecternDefault"
  ${EndIf}
  ${If} $LecternDefault != "=Lectern.Markdown"
    WriteRegStr SHCTX "Software\Classes\.${EXT}" "Lectern.Markdown_original" $LecternDefault
  ${EndIf}
!macroend

!macro LECTERN_OPEN_WITH EXT
  WriteRegStr SHCTX "Software\Classes\.${EXT}\OpenWithProgids" "Lectern.Markdown" ""
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe\SupportedTypes" ".${EXT}" ""
!macroend

; Puts the recorded default back and removes what the installer added to the extension.
!macro LECTERN_RESTORE_DEFAULT EXT
  ClearErrors
  ReadRegStr $LecternDefault SHCTX "Software\Classes\.${EXT}" "Lectern.Markdown_original"
  ${If} ${Errors}
    ; Nothing recorded: at least don't leave the extension on the class just deleted.
    ReadRegStr $LecternDefault SHCTX "Software\Classes\.${EXT}" ""
    ${If} $LecternDefault == "Lectern.Markdown"
      DeleteRegValue SHCTX "Software\Classes\.${EXT}" ""
    ${EndIf}
  ${ElseIf} $LecternDefault == "none"
    DeleteRegValue SHCTX "Software\Classes\.${EXT}" ""
  ${Else}
    StrCpy $LecternDefault $LecternDefault "" 1
    WriteRegStr SHCTX "Software\Classes\.${EXT}" "" $LecternDefault
  ${EndIf}
  DeleteRegValue SHCTX "Software\Classes\.${EXT}" "Lectern.Markdown_original"
  DeleteRegValue SHCTX "Software\Classes\.${EXT}" "Lectern.Markdown_backup"
  DeleteRegValue SHCTX "Software\Classes\.${EXT}\OpenWithProgids" "Lectern.Markdown"
  DeleteRegKey /ifempty SHCTX "Software\Classes\.${EXT}\OpenWithProgids"
  DeleteRegKey /ifempty SHCTX "Software\Classes\.${EXT}"
  ; Explorer's own per-user list of the classes it has offered for the extension.
  DeleteRegValue HKCU "${LECTERN_FILEEXTS}\.${EXT}\OpenWithProgids" "Lectern.Markdown"
  DeleteRegKey /ifempty HKCU "${LECTERN_FILEEXTS}\.${EXT}\OpenWithProgids"
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro LECTERN_REMEMBER_DEFAULT "md"
  !insertmacro LECTERN_REMEMBER_DEFAULT "markdown"
  !insertmacro LECTERN_REMEMBER_DEFAULT "mdown"
  !insertmacro LECTERN_REMEMBER_DEFAULT "mkd"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr SHCTX "Software\Classes\Lectern.Markdown\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  WriteRegStr SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe\shell\open\command" "" '"$INSTDIR\${MAINBINARYNAME}.exe" "%1"'
  !insertmacro LECTERN_OPEN_WITH "md"
  !insertmacro LECTERN_OPEN_WITH "markdown"
  !insertmacro LECTERN_OPEN_WITH "mdown"
  !insertmacro LECTERN_OPEN_WITH "mkd"
  !insertmacro UPDATEFILEASSOC
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Another app may have taken an extension since: that is what to restore then.
  !insertmacro LECTERN_REMEMBER_DEFAULT "md"
  !insertmacro LECTERN_REMEMBER_DEFAULT "markdown"
  !insertmacro LECTERN_REMEMBER_DEFAULT "mdown"
  !insertmacro LECTERN_REMEMBER_DEFAULT "mkd"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro LECTERN_RESTORE_DEFAULT "md"
  !insertmacro LECTERN_RESTORE_DEFAULT "markdown"
  !insertmacro LECTERN_RESTORE_DEFAULT "mdown"
  !insertmacro LECTERN_RESTORE_DEFAULT "mkd"
  DeleteRegKey SHCTX "Software\Classes\Applications\${MAINBINARYNAME}.exe"
  ; Tauri keeps the install folder here for a later reinstall; an uninstall that isn't part of an
  ; update leaves nothing behind.
  ${If} $UpdateMode <> 1
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
  ${EndIf}
  !insertmacro UPDATEFILEASSOC
!macroend
