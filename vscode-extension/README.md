# GitSwitch for VS Code

Shows **which git identity applies to the folder you're working in** — and warns
you *before* you commit as the wrong account.

Companion to the [GitSwitch desktop app](https://github.com/Kshitijkb28/GitSwitch). The app manages your
profiles and the `includeIf` rules in `~/.gitconfig`; this extension reads the
same `profiles.json` and asks git what it actually resolved, so the two can
never disagree.

## What you get

- **Status bar** — the profile and identity for the current repo, plus `↑ahead`
  / `↓behind` counts.
- **Mismatch warning** — if the repo commits as one address while its GitSwitch
  profile says another, the status bar turns amber. This is the "I just
  committed to work with my personal account" mistake, caught before it happens.
- **Push-blocked / behind indicators** in the tooltip.
- **`GitSwitch: Check for incoming commits`** — runs `git fetch` (never `pull`),
  so you can see what's waiting without touching your working tree.

## Commands

| Command | What it does |
|---|---|
| `GitSwitch: Show identity for this folder` | Full detail — repo, branch, identity, profile, SSH key, sync |
| `GitSwitch: Check for incoming commits (fetch)` | Fetches and reports how far behind you are |
| `GitSwitch: Refresh identity` | Re-reads git and profiles.json |

## Settings

- `gitswitch.showStatusBar` (default `true`)
- `gitswitch.warnOnMismatch` (default `true`)

## Install

```bash
code --install-extension gitswitch-0.1.0.vsix
```

Requires `git` on PATH. The desktop app is optional — without it you still get
the identity readout, just no profile comparison.
