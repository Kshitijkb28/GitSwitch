; GitSwitch's additions to Tauri's NSIS installer.
;
; Uninstalling takes away the "start when I sign in" entry (the Net speed
; tab's choice): a Run value naming a program that is gone, and Startup apps'
; on/off for it. Not during an update: the old version's uninstaller runs
; with /UPDATE, and the entry must survive it. GitSwitch also puts the entry
; back at its next start whenever the choice is on.
;
; HKCU is the hive of whoever runs the uninstaller. For the person who
; installed GitSwitch that is their own; GitSwitch's own uninstall (the
; Uninstall tab) has already taken the entry away, as that person.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "GitSwitch"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "GitSwitch"
  ${EndIf}
!macroend
