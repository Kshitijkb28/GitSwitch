# SSH Setup Guide

A step-by-step guide to setting up SSH authentication so you can clone, push, and pull from a Git host (GitHub, GitLab, Bitbucket, etc.) without typing a username/password each time.

Works on **macOS**, **Linux**, and **Windows**. Where commands differ, both are shown.

---

## Step 1 — Open your terminal

- **macOS / Linux:** open the **Terminal** app.
- **Windows:** open **Git Bash** (recommended, installed with Git for Windows), **PowerShell**, or **Windows Terminal**.

---

## Step 2 — Go to your SSH directory

```bash
cd ~/.ssh
```

- `cd` — "change directory".
- `~` — shortcut for your home folder (`/Users/<you>` on macOS, `/home/<you>` on Linux, `C:\Users\<you>` on Windows).
- `.ssh` — the hidden folder that stores your SSH keys and config.

> If the folder does not exist yet, create it first:
> ```bash
> mkdir -p ~/.ssh
> ```
> `mkdir` makes a directory; `-p` avoids an error if it already exists.

---

## Step 3 — Check for existing keys

```bash
ls -a
```

- `ls` — lists files in the current directory.
- `-a` — shows **all** files, including hidden ones (SSH files are hidden by default).

Look for a key pair such as `id_ed25519` (private) and `id_ed25519.pub` (public). If they already exist and you want to reuse them, you can skip to **Step 5**.

> **Note:** The ssh-agent (which keeps a key unlocked in memory) is only needed when your key has a passphrase. If you created the key **without a passphrase**, no agent step is required.

---

## Step 4 — Generate a new SSH key

```bash
ssh-keygen -t ed25519 -C "your_email@example.com"
```

- `ssh-keygen` — the tool that generates SSH key pairs.
- `-t ed25519` — the key **type/algorithm**. `ed25519` is modern, fast, and secure.
  - If your Git host is very old and doesn't support it, use: `ssh-keygen -t rsa -b 4096 -C "your_email@example.com"`
- `-C "your_email@example.com"` — a **comment label** to help you identify the key later. Use the email of the GitHub account that has your company/organization access.

You'll be asked three questions. **Just press the Enter key for each of them** — you don't need to type anything:

```
Enter file in which to save the key (.../.ssh/id_ed25519):   ← press Enter
Enter passphrase (empty for no passphrase):                  ← press Enter
Enter same passphrase again:                                 ← press Enter
```

1. **File to save the key** — press **Enter** to accept the default location (`~/.ssh/id_ed25519`).
2. **Passphrase** — press **Enter** to leave it empty (no password). *(You may type one for extra security, but then you'll be asked for it each time.)*
3. **Same passphrase again** — press **Enter** to confirm the empty passphrase.

This creates **two files**:

| File | Meaning | Share it? |
|------|---------|-----------|
| `id_ed25519` | **Private** key | ❌ Never — keep it secret |
| `id_ed25519.pub` | **Public** key | ✅ Yes — this is what you give to the Git host |

Confirm they were created:

```bash
ls -a
```

You should now see `id_ed25519` and `id_ed25519.pub`.

---

## Step 5 — Copy your public key

You'll paste this into your Git account in the next step.

**macOS:**
```bash
pbcopy < ~/.ssh/id_ed25519.pub
```

**Windows (Git Bash):**
```bash
clip < ~/.ssh/id_ed25519.pub
```

**Linux (with xclip installed):**
```bash
xclip -selection clipboard < ~/.ssh/id_ed25519.pub
```

**Any system — just print it and copy manually:**
```bash
cat ~/.ssh/id_ed25519.pub
```

- `cat` — prints a file's contents to the screen.
- The output starts with `ssh-ed25519 ...` and ends with your comment label. Copy the **entire line**.

> ⚠️ Always copy the `.pub` (public) file. **Never** share the private key file.

---

## Step 6 — Add the SSH key to your Git account

This step is done in the **website UI**, not the terminal.

### GitHub
1. Go to **GitHub** and sign in.
2. Click your profile photo (top-right) → **Settings**.
3. In the left sidebar, click **SSH and GPG keys**.
4. Click **New SSH key**.
5. **Title** — any name to recognize this device (e.g. "My Laptop").
6. **Key type** — leave as **Authentication Key**.
7. **Key** — paste the public key you copied in Step 5.
8. Click **Add SSH key** (confirm your password if prompted).

---

## Step 7 — Test the connection

```bash
ssh -T git@github.com
```

- `ssh` — starts an SSH connection.
- `-T` — disables terminal allocation (the Git host doesn't give you a shell, so we skip it).
- `git@github.com` — the SSH user (`git`) and host.

The first time, you'll be asked to trust the host's fingerprint — type **`yes`** and press Enter.

A successful result looks like:
```
Hi <your-username>! You've successfully authenticated, but GitHub does not provide shell access.
```

For other hosts:
- **GitLab:** `ssh -T git@gitlab.com`
- **Bitbucket:** `ssh -T git@bitbucket.org`

---

## Step 8 — Clone a repository over SSH

```bash
git clone git@github.com:USERNAME/REPOSITORY.git
```

- `git clone` — downloads a copy of the repository to your machine.
- Use the **SSH URL** (`git@github.com:...`), **not** the HTTPS URL (`https://github.com/...`).
- On the repo's page, click **Code → SSH** to copy the correct URL.

---

## Quick Reference

| Task | Command |
|------|---------|
| Go to SSH folder | `cd ~/.ssh` |
| List all files | `ls -a` |
| Generate key | `ssh-keygen -t ed25519 -C "your_email@example.com"` |
| Add key to agent | `ssh-add ~/.ssh/id_ed25519` |
| Show public key | `cat ~/.ssh/id_ed25519.pub` |
| Test connection | `ssh -T git@github.com` |
| Clone repo | `git clone git@github.com:USERNAME/REPO.git` |

---

## Key Takeaways

- The **private** key (`id_ed25519`) never leaves your computer.
- Only the **public** key (`id_ed25519.pub`) is added to your Git account.
- One key pair can be used with multiple Git hosts.
- If you get a "Permission denied (publickey)" error, re-check Step 5 (agent) and Step 7 (key added correctly).
