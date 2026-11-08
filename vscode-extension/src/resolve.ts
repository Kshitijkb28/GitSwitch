import * as fs from "fs";
import * as os from "os";
import * as path from "path";

/**
 * Which GitSwitch profile a repository commits as, decided the way the app
 * decides it — and free of the `vscode` module, so it can be tested on its
 * own and run by the verification suites against a real git config.
 *
 * The app writes the profiles into ~/.gitconfig as include rules: a folder
 * rule (`includeIf "gitdir:…"`) per profile folder, a remote rule
 * (`includeIf "hasconfig:remote.*.url:…"`) per owner/repo pattern, and the
 * default profile as the global [user]. A repository is therefore the folder
 * profile's when it lies in one of its folders (the longest folder wins);
 * otherwise the profile whose include file git read the email from — a clone
 * of a remote-rule owner anywhere on disk; otherwise the default profile.
 */

export interface Profile {
  id: string;
  name: string;
  git_name: string;
  git_email: string;
  ssh_key_path: string | null;
  is_default: boolean;
  allow_push: boolean;
  signing_enabled?: boolean;
  directories: string[];
  remote_patterns?: string[];
}

/** Where the app stores its data — Rust's `dirs::data_dir()`. */
export function profilesPath(
  home: string = os.homedir(),
  platform: string = process.platform,
  env: NodeJS.ProcessEnv = process.env
): string {
  // The paths of the platform asked about, not of the one this runs on: the
  // tests ask about all three from any of them.
  const p = platform === "win32" ? path.win32 : path.posix;
  // dirs ignores XDG_DATA_HOME unless it is absolute; so does this.
  const xdg = env.XDG_DATA_HOME && p.isAbsolute(env.XDG_DATA_HOME) ? env.XDG_DATA_HOME : undefined;
  const dir =
    platform === "darwin"
      ? p.join(home, "Library", "Application Support")
      : platform === "win32"
        ? env.APPDATA ?? p.join(home, "AppData", "Roaming")
        : xdg ?? p.join(home, ".local", "share");
  return p.join(dir, "com.gitswitch.app", "profiles.json");
}

export function loadProfiles(file: string = profilesPath()): Profile[] {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8")).profiles ?? [];
  } catch {
    return [];
  }
}

/** Windows mixes \ and /; compare paths without a separator style. */
export function norm(p: string): string {
  const s = p.replace(/\\/g, "/").replace(/\/+$/, "");
  return s || p;
}

export function isWithin(child: string, parent: string): boolean {
  const c = norm(child);
  const p = norm(parent);
  return c === p || c.startsWith(`${p}/`);
}

/** The folder profile: the longest of its folders the repository is in. */
export function folderProfile(repo: string, profiles: Profile[]): Profile | null {
  let best: Profile | null = null;
  let bestLen = -1;
  for (const p of profiles) {
    for (const d of p.directories) {
      const dir = norm(d);
      if (isWithin(repo, dir) && dir.length > bestLen) {
        best = p;
        bestLen = dir.length;
      }
    }
  }
  return best;
}

/** `git config --null --show-origin --get-all user.email`: origin NUL value NUL, repeated. */
export function originPairs(out: string): Array<[string, string]> {
  const parts = out.split("\0");
  const pairs: Array<[string, string]> = [];
  for (let i = 0; i + 1 < parts.length; i += 2) {
    if (parts[i]) pairs.push([parts[i], parts[i + 1]]);
  }
  return pairs;
}

/**
 * The profile whose include file git read the email from: `<id>.gitconfig`
 * in the app's data folder. From the last value back, so a repository's own
 * .git/config, read after the includes, does not hide the profile it is in.
 */
export function profileFromOrigins(pairs: Array<[string, string]>, profiles: Profile[]): Profile | null {
  for (let i = pairs.length - 1; i >= 0; i--) {
    const file = pairs[i][0].replace(/^file:/, "");
    const base = file.split(/[\\/]/).pop() ?? "";
    const id = base.endsWith(".gitconfig") ? base.slice(0, -".gitconfig".length) : "";
    const found = id ? profiles.find((p) => p.id === id) : undefined;
    if (found) return found;
  }
  return null;
}

/** git compares emails as written; GitHub and people do not mind letter case. */
export function emailsMatch(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

export interface Resolved {
  name: string;
  email: string;
  profile: Profile | null;
  /** git commits as someone else than the profile that owns this repository. */
  mismatch: boolean;
}

export type Git = (cwd: string, args: string[]) => Promise<string>;

export async function resolveIdentity(repoRoot: string, git: Git, profiles: Profile[] = loadProfiles()): Promise<Resolved> {
  const [name, email, origins] = await Promise.all([
    git(repoRoot, ["config", "user.name"]).catch(() => ""),
    git(repoRoot, ["config", "user.email"]).catch(() => ""),
    git(repoRoot, ["config", "--null", "--show-origin", "--get-all", "user.email"]).catch(() => ""),
  ]);
  const profile =
    folderProfile(repoRoot, profiles) ??
    profileFromOrigins(originPairs(origins), profiles) ??
    profiles.find((p) => p.is_default) ??
    null;
  return { name, email, profile, mismatch: !!profile && !!email && !emailsMatch(profile.git_email, email) };
}
