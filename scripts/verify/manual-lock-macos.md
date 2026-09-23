# Push lock — manual check on macOS (real administrator prompt)

Everything else about the lock is proven unprivileged by `lock.sh`. Only the
operating system's prompt cannot be scripted, so this runs once, by hand, with
you present, on a **throwaway** repository — never a real one.

Preconditions: the app built from this tree and installed
(`scripts/install-mac.sh`), no `/etc/gitswitch` yet (`ls /etc/gitswitch` →
"No such file"), Terminal open.

```bash
T=$(mktemp -d); git init -q --bare "$T/remote.git"
git clone -q "$T/remote.git" "$T/work"; cd "$T/work"
echo a > a.txt; git add -A; git commit -qm init; git push -q -u origin main
echo "$T/work"   # paste into the app's Changes page repo picker
```

| # | Step | Expect | Result |
|---|---|---|---|
| 1 | Changes → Push access → **Locked** | A confirmation panel names the repo path, says macOS will ask for an administrator name and password, and (first time) describes the helper install with its SHA-256. Nothing has run. | |
| 2 | Click **Lock (administrator prompt)**, then **Cancel** in the macOS dialog | Card shows "Cancelled. You cancelled the administrator prompt. Nothing changed." Segment still Allowed. `ls /etc/gitswitch` still "No such file". | |
| 3 | Again **Locked** → **Lock (administrator prompt)**, enter the password | Dialog text is GitSwitch's ("GitSwitch needs an administrator to change a push lock."). Card: "Locked 1 repository. Lock helper installed." Segment Locked; layers all green; "Locked <time>". | |
| 4 | In Terminal: `ls -la /etc/gitswitch /etc/gitswitch/locks.d /Library/PrivilegedHelperTools/com.gitswitch.lock-helper /usr/local/bin/git-remote-gitswitch-push-blocked; cat /etc/gitconfig` | Everything `root wheel`; `locks.json` 0644; helper 0755; the marker block in `/etc/gitconfig` includes `/etc/gitswitch/locks.gitconfig`. Record the output. | |
| 5 | `echo b >> a.txt; git commit -qam b; git push origin main` | Refused: "GitSwitch: push refused." … "Do not work around this; ask the owner to unlock it, which requires their administrator password." … `gitswitch: lock=on policy=ask-owner`. Exit 128. | |
| 6 | `git push --no-verify origin main` | Same refusal (this is not a hook). | |
| 7 | `git config --local --unset gitswitch.pushBlocked; rm .git/hooks/pre-push`, then refresh the Changes page | Mirrors line says GitSwitch put them back just now; Recent events shows `mirrors-drifted … re-applied`. `git config --local gitswitch.pushBlocked` → `true`. | |
| 8 | `sudo -k` (drop any sudo timestamp), then in the app **Allowed** → **Unlock (administrator prompt)** within 5 minutes of step 3 | The macOS prompt appears **again** (a different job file and checksum, so the 5-minute approval cache does not apply). | |
| 9 | Enter the password | "Unlocked 1 repository." Segment Allowed. `git push origin main` succeeds. `cat /etc/gitconfig` no longer has the marker block (or the file is gone if it held nothing else). | |
| 10 | Doctor → Run checks | No push-lock findings. | |
| 11 | Settings → Push locks → **Remove push locks and helper…** → **Remove (administrator prompt)** → password | "Push locks and the lock helper were removed." `ls /etc/gitswitch` → "No such file"; the helper and the remote-helper symlink are gone. | |
| 12 | Cleanup: `rm -rf "$T"` | | |

Record the run: date, macOS version, git version (`git --version`), and the
`ls -la` output from step 4, at the bottom of this file.

## Runs

_(none yet)_
