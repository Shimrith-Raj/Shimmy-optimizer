; Shimmy Optimizer installer hooks (Tauri NSIS). The installer runs elevated (perMachine), like Shimmy itself.

; Tauri leaves CompanyName empty in the installer's file properties.
VIAddVersionKey "CompanyName" "Shimrith Raj"

!macro NSIS_HOOK_POSTINSTALL
  ; PawnIO, the signed driver LibreHardwareMonitor uses for CPU temperature, clocks and power. It is only installed
  ; when its service is missing, so a PC that already has it is left alone. It stays installed if Shimmy is removed
  ; (it is listed in Apps on its own).
  SetRegView 64
  ClearErrors
  ReadRegStr $R0 HKLM "SYSTEM\CurrentControlSet\Services\PawnIO" "ImagePath"
  IfErrors pawnio_install pawnio_present
  pawnio_present:
    DetailPrint "The PawnIO sensor driver is already installed, leaving it as it is"
    Goto pawnio_done
  pawnio_install:
    DetailPrint "Installing the PawnIO sensor driver"
    ExecWait '"$INSTDIR\pawnio\PawnIO_setup.exe" -install -silent' $R0
    StrCmp $R0 "0" pawnio_done
    DetailPrint "Warning: the PawnIO installer exited with code $R0, so CPU temperatures may be unavailable"
  pawnio_done:
  ; Start with Windows through a Task Scheduler task with highest privileges, so there is no UAC prompt at sign-in.
  DetailPrint "Starting Shimmy with Windows"
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --autostart on'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --autostart off'
  ; Nothing manages the charge limit once Shimmy is gone, so one it set must not keep the battery capped. An update
  ; passes through here too; the new version writes the limit again as it starts.
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --reset-limit'
!macroend
