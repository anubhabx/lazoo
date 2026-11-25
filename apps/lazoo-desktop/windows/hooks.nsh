; Lazoo installer hooks (bundle.windows.nsis.installerHooks).
; Before files are replaced or removed, stop the local service that runs from this install:
; ask the daemon to shut down, then end any keeper, daemon or session host still running from
; $INSTDIR. Processes from other locations (development builds) are left alone. Data under
; %LOCALAPPDATA%\Lazoo is never touched.

!macro LAZOO_STOP_SERVICE
  ${If} ${FileExists} "$INSTDIR\lazooctl.exe"
    DetailPrint "Stopping the Lazoo service..."
    nsExec::Exec '"$INSTDIR\lazooctl.exe" system shutdown'
    Pop $0
    Sleep 1500
    nsExec::Exec `powershell -NoProfile -NonInteractive -Command "Get-Process lazoo-keeper,lazood,lazoo-session-host -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' } | Stop-Process -Force"`
    Pop $0
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro LAZOO_STOP_SERVICE
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro LAZOO_STOP_SERVICE
!macroend
