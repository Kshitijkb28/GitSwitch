# Push lock — manual check on Linux (real prompt)

CI proves the helper as real root on an Ubuntu runner (`LOCK_REAL=1`), so
file ownership, `/etc/gitconfig` editing, the polkit policy file and the
refusal itself are covered. What CI cannot show is the **dialog**: polkit's
agent on a desktop, and the fallbacks when there is none. Do this once, by
hand, on a throwaway repository — never a real one.

Preconditions: the app built from this tree and installed; `ls /etc/gitswitch`
→ "No such file"; a desktop session (GNOME/KDE) for the first half, an ssh
session for the second.

```bash
T=$(mktemp -d); git init -q --bare "$T/remote.git"
git clone -q "$T/remote.git" "$T/work"; cd "$T/work"
echo a > a.txt; git add -A; git commit -qm init; git push -q -u origin main
echo "$T/work"   # paste into the Changes page repo picker
```

| # | Step | Expect | Result |
|---|---|---|---|
| 1 | Changes → Push access → **Locked** → **Lock (administrator prompt)** | polkit's agent asks for the administrator password with the text "GitSwitch needs an administrator to change a push lock." | |
| 2 | Cancel the dialog | "Cancelled. … Nothing changed." `ls /etc/gitswitch` still empty. | |
| 3 | Again, enter the password | "Locked 1 repository. Lock helper installed." `ls -la /etc/gitswitch /usr/local/lib/gitswitch /usr/local/bin/git-remote-gitswitch-push-blocked /usr/share/polkit-1/actions/com.gitswitch.lock-helper.policy` all `root root`. Record the output. | |
| 4 | `git push origin main` in a terminal, then with `--no-verify` | Both refused with "GitSwitch: push refused." … "Do not work around this …". | |
| 5 | **Allowed** → **Unlock (administrator prompt)** → password | The dialog appears again (no `auth_admin_keep`); "Unlocked 1 repository."; push works. | |
| 6 | Over **ssh** (no polkit agent): **Locked** → prompt | If `ssh-askpass` or `SUDO_ASKPASS` exists, `sudo -A` asks; otherwise the card shows the exact `sudo /usr/local/lib/gitswitch/lock-helper … --sha256 …` command with **I ran it**. Run it, press the button: "Locked 1 repository." | |
| 7 | Settings → Push locks → Remove → password | Everything gone: `ls /etc/gitswitch` → "No such file"; policy file and helper removed. | |
| 8 | `rm -rf "$T"` | | |

## Runs

_(none yet)_
