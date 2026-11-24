; Hooks for the NSIS installer (bundle.windows.nsis.installerHooks in tauri.conf.json).

; Windows caches shortcut icons by the exe's path, and an update replaces the exe at the
; same path, so Start, the desktop and the taskbar would keep showing an old icon.
; SHCNE_ASSOCCHANGED (0x08000000) tells the shell to reload them; SHCNF_FLUSH (0x1000)
; waits until it has.
!macro NSIS_HOOK_POSTINSTALL
  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, p 0, p 0)"
!macroend
