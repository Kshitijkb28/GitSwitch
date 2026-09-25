# Push lock — manual check on Windows (real UAC prompt)

CI proves the helper elevated on a Windows runner (`LOCK_REAL=1`): the
Administrators-only DACL on `C:\ProgramData\GitSwitch\lock`, the registry
lookup of Git for Windows, the marker block in `<InstallPath>\etc\gitconfig`,
the `.exe` remote helper in git's exec path, and `git push` refused. The runner
has UAC **off**, so the one thing left to see by hand is the prompt itself.
Throwaway repository only.

Preconditions: the app installed; Git for Windows; `dir C:\ProgramData\GitSwitch`
→ not found. Do the first half on an **administrator** account and the second
on a **standard** account (Doctor names which kind you have).

```powershell
$T = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ("gs-" + [guid]::NewGuid()))
git init -q --bare "$T\remote.git"; git clone -q "$T\remote.git" "$T\work"; cd "$T\work"
"a" | Set-Content a.txt; git add -A; git commit -qm init; git push -q -u origin main
"$T\work"   # paste into the Changes page repo picker
```

| # | Step | Expect | Result |
|---|---|---|---|
| 1 | Changes → Push access → **Locked** → **Lock (administrator prompt)** | UAC dialog for `gitswitch-lock-helper.exe`. On an administrator account it is a **Yes/No** consent, not a password — the card's caveats say so, and Doctor shows `push-lock-uac-consent`. | |
| 2 | Click **No** | "Cancelled. … Nothing changed." | |
| 3 | Again, click **Yes** | "Locked 1 repository. Lock helper installed." `icacls C:\ProgramData\GitSwitch\lock` shows `BUILTIN\Administrators:(OI)(CI)(F)`, `NT AUTHORITY\SYSTEM:(OI)(CI)(F)`, `BUILTIN\Users:(OI)(CI)(RX)` and nothing writable by Users. `type "C:\Program Files\Git\etc\gitconfig"` shows the marker block. Record both. | |
| 4 | `git push origin main`, then `git push --no-verify origin main` (Git Bash **and** PowerShell) | Both refused with "GitSwitch: push refused." … "Do not work around this …". | |
| 5 | **Allowed** → **Unlock** → **Yes** | "Unlocked 1 repository."; push works; the marker block is gone. | |
| 6 | Standard account: **Locked** | UAC asks for an **administrator's password** this time. | |
| 7 | Settings → Push locks → Remove → **Yes** | `dir C:\ProgramData\GitSwitch` → not found; the remote helper `.exe` is gone from `<InstallPath>\mingw64\libexec\git-core`; the helper itself disappears at the next reboot (a running exe cannot delete itself). | |
| 8 | With UAC set to "Never notify" (then set it back!): Doctor | `push-lock-uac-off` warning. | |

## Runs

_(none yet)_
