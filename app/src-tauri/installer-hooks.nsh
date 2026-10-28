; Shimmy Optimizer installer hooks (Tauri NSIS). The installer runs elevated (perMachine), like Shimmy itself.

!macro NSIS_HOOK_POSTINSTALL
  ; PawnIO, the signed driver LibreHardwareMonitor uses for CPU temperature, clocks and power. Re-running it on a PC
  ; that already has it is harmless. It stays installed if Shimmy is removed (it is listed in Apps on its own).
  DetailPrint "Installing the PawnIO sensor driver"
  ExecWait '"$INSTDIR\pawnio\PawnIO_setup.exe" -install -silent'
  ; Start with Windows through a Task Scheduler task with highest privileges, so there is no UAC prompt at sign-in.
  DetailPrint "Starting Shimmy with Windows"
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --autostart on'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --autostart off'
!macroend
