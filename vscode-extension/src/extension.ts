import * as vscode from "vscode";
import { execFile } from "child_process";
import { promisify } from "util";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

const run = promisify(execFile);

/**
 * The extension deliberately does NOT reimplement GitSwitch's logic.
 *
 * The desktop app owns the profiles and writes the includeIf rules into
 * ~/.gitconfig; git itself then resolves the identity. So the extension only
 * has to READ the same profiles.json and ASK git what it resolved — there is
 * one source of truth and the two can never drift apart.
 */

interface Profile {
  id: string;
  name: string;
  git_name: string;
  git_email: string;
  ssh_key_path: string | null;
  is_default: boolean;
  allow_push: boolean;
  signing_enabled?: boolean;
  directories: string[];
}

/** Where the Tauri app stores its data (mirrors Rust's dirs::data_dir()). */
function profilesPath(): string {
  const home = os.homedir();
  const dir =
    process.platform === "darwin"
      ? path.join(home, "Library", "Application Support")
      : process.platform === "win32"
      ? process.env.APPDATA ?? path.join(home, "AppData", "Roaming")
      : process.env.XDG_DATA_HOME ?? path.join(home, ".local", "share");
  return path.join(dir, "com.gitswitch.app", "profiles.json");
}

function loadProfiles(): Profile[] {
  try {
    const raw = fs.readFileSync(profilesPath(), "utf8");
    return JSON.parse(raw).profiles ?? [];
  } catch {
    return [];
  }
}

/** Windows mixes \ and /; compare paths without a separator style. */
function norm(p: string): string {
  const s = p.replace(/\\/g, "/").replace(/\/+$/, "");
  return s || p;
}
function isWithin(child: string, parent: string): boolean {
  const c = norm(child);
  const p = norm(parent);
  return c === p || c.startsWith(`${p}/`);
}

/** Longest matching folder wins — same rule the generated gitconfig follows. */
function profileFor(repoPath: string, profiles: Profile[]): Profile | null {
  let best: Profile | null = null;
  let bestLen = -1;
  for (const p of profiles) {
    for (const d of p.directories) {
      const dir = norm(d);
      if (isWithin(repoPath, dir) && dir.length > bestLen) {
        best = p;
        bestLen = dir.length;
      }
    }
  }
  return best ?? profiles.find((p) => p.is_default) ?? null;
}

async function git(cwd: string, args: string[]): Promise<string> {
  const { stdout } = await run("git", args, { cwd, timeout: 10_000 });
  return stdout.trim();
}

interface Identity {
  repoRoot: string;
  name: string;
  email: string;
  profile: Profile | null;
  /** git's answer disagrees with the profile that owns this folder */
  mismatch: boolean;
  ahead: number;
  behind: number;
  branch: string;
}

async function currentIdentity(): Promise<Identity | null> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) return null;
  const cwd = folder.uri.fsPath;

  let repoRoot: string;
  try {
    repoRoot = await git(cwd, ["rev-parse", "--show-toplevel"]);
  } catch {
    return null; // not a git repo — nothing to say
  }

  const [name, email, branch] = await Promise.all([
    git(repoRoot, ["config", "user.name"]).catch(() => ""),
    git(repoRoot, ["config", "user.email"]).catch(() => ""),
    git(repoRoot, ["rev-parse", "--abbrev-ref", "HEAD"]).catch(() => ""),
  ]);

  let ahead = 0;
  let behind = 0;
  try {
    const counts = await git(repoRoot, [
      "rev-list",
      "--left-right",
      "--count",
      `${branch}...${branch}@{upstream}`,
    ]);
    const [a, b] = counts.split(/\s+/);
    ahead = parseInt(a, 10) || 0;
    behind = parseInt(b, 10) || 0;
  } catch {
    /* no upstream */
  }

  const profile = profileFor(repoRoot, loadProfiles());
  return {
    repoRoot,
    name,
    email,
    branch,
    profile,
    ahead,
    behind,
    // Only a real conflict counts: a profile exists AND git resolved something else.
    mismatch: !!profile && !!email && profile.git_email.trim() !== email.trim(),
  };
}

let statusItem: vscode.StatusBarItem;
let current: Identity | null = null;

async function refresh(): Promise<void> {
  current = await currentIdentity();
  const show = vscode.workspace.getConfiguration("gitswitch").get<boolean>("showStatusBar", true);
  if (!current || !show) {
    statusItem.hide();
    return;
  }

  const warn =
    current.mismatch &&
    vscode.workspace.getConfiguration("gitswitch").get<boolean>("warnOnMismatch", true);

  const label = current.profile?.name ?? "no profile";
  const behind = current.behind > 0 ? `  ↓${current.behind}` : "";
  const ahead = current.ahead > 0 ? `  ↑${current.ahead}` : "";

  statusItem.text = warn
    ? `$(alert) ${current.email}${ahead}${behind}`
    : `$(git-branch) ${label}${ahead}${behind}`;

  statusItem.tooltip = new vscode.MarkdownString(
    [
      `**GitSwitch**`,
      ``,
      `Committing as **${current.name || "(unset)"}** \`${current.email || "(unset)"}\``,
      current.profile
        ? `Profile for this folder: **${current.profile.name}** \`${current.profile.git_email}\``
        : `No GitSwitch profile covers this folder — git is falling back to your global identity.`,
      warn
        ? `\n⚠️ **Mismatch** — this repo commits as \`${current.email}\` but the profile says \`${current.profile!.git_email}\`.`
        : ``,
      current.profile && current.profile.allow_push === false
        ? `\n🚫 Pushing is blocked for this profile.`
        : ``,
      current.behind > 0
        ? `\n↓ ${current.behind} commit(s) on the remote you haven't pulled.`
        : ``,
      `\n_Click for details._`,
    ]
      .filter(Boolean)
      .join("\n")
  );

  statusItem.backgroundColor = warn
    ? new vscode.ThemeColor("statusBarItem.warningBackground")
    : undefined;
  statusItem.command = "gitswitch.showIdentity";
  statusItem.show();
}

async function showIdentity(): Promise<void> {
  await refresh();
  if (!current) {
    vscode.window.showInformationMessage("GitSwitch: this workspace isn't a git repository.");
    return;
  }
  const lines = [
    `Repo:      ${current.repoRoot}`,
    `Branch:    ${current.branch}`,
    `Commits as ${current.name || "(unset)"} <${current.email || "(unset)"}>`,
    current.profile
      ? `Profile:   ${current.profile.name} <${current.profile.git_email}>` +
        (current.profile.ssh_key_path ? `\nSSH key:   ${current.profile.ssh_key_path}` : "")
      : `Profile:   none covers this folder`,
    current.ahead || current.behind ? `Sync:      ↑${current.ahead} ahead, ↓${current.behind} behind` : "",
  ].filter(Boolean);

  const action = current.mismatch ? "Fix in GitSwitch" : "Open GitSwitch";
  const picked = await vscode.window.showInformationMessage(
    current.mismatch
      ? `⚠️ This repo commits as ${current.email}, but its GitSwitch profile is ${current.profile!.git_email}.`
      : `Committing as ${current.email}`,
    { modal: true, detail: lines.join("\n") },
    action
  );
  if (picked) {
    vscode.env.openExternal(vscode.Uri.parse("gitswitch://profiles")).then(undefined, () => {
      vscode.window.showWarningMessage("Couldn't launch the GitSwitch app — open it manually.");
    });
  }
}

/** fetch, never pull: refreshes remote refs without touching the working tree. */
async function fetchStatus(): Promise<void> {
  const id = await currentIdentity();
  if (!id) {
    vscode.window.showInformationMessage("GitSwitch: this workspace isn't a git repository.");
    return;
  }
  await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Window, title: "GitSwitch: fetching…" },
    async () => {
      try {
        await git(id.repoRoot, ["fetch", "--all", "--quiet"]);
      } catch (e) {
        vscode.window.showErrorMessage(`GitSwitch: fetch failed — ${e}`);
        return;
      }
      await refresh();
      const behind = current?.behind ?? 0;
      vscode.window.showInformationMessage(
        behind > 0
          ? `${behind} commit(s) on the remote you haven't pulled. Your working tree was not touched.`
          : "Up to date with the remote."
      );
    }
  );
}

export function activate(context: vscode.ExtensionContext) {
  statusItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  context.subscriptions.push(statusItem);

  context.subscriptions.push(
    vscode.commands.registerCommand("gitswitch.showIdentity", showIdentity),
    vscode.commands.registerCommand("gitswitch.refresh", refresh),
    vscode.commands.registerCommand("gitswitch.fetchStatus", fetchStatus),
    // Re-check whenever the picture could have changed.
    vscode.window.onDidChangeActiveTextEditor(() => refresh()),
    vscode.window.onDidChangeWindowState((s) => s.focused && refresh()),
    vscode.workspace.onDidChangeWorkspaceFolders(() => refresh())
  );

  // The desktop app rewrites profiles.json; pick changes up without a reload.
  try {
    const watcher = fs.watch(path.dirname(profilesPath()), () => refresh());
    context.subscriptions.push({ dispose: () => watcher.close() });
  } catch {
    /* app not installed yet — the status bar simply shows no profile */
  }

  refresh();
}

export function deactivate() {
  statusItem?.dispose();
}
