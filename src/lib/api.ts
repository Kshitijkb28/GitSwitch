import { invoke as tauriInvoke, Channel } from "@tauri-apps/api/core";
import type { Profile } from "../types/profile";

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (typeof window === "undefined" || !(window as any).__TAURI_INTERNALS__) {
    return Promise.reject(new Error(`Tauri runtime not available (command: ${cmd})`));
  }
  return tauriInvoke<T>(cmd, args);
}

export async function getProfiles(): Promise<Profile[]> {
  return invoke("get_profiles");
}

/** The page a gitswitch:// link asked for before the page listened (the link that started the app). */
export async function deepLinkTake(): Promise<string | null> {
  return invoke("deep_link_take");
}

// --- Ports: every listening port, who holds it, and stopping it --------------

/** The repository in a profile folder a process runs inside. */
export interface CloneRef {
  path: string;
  name: string;
  profile_name: string;
}

/** The container a port is published by, and its Compose project when it has one. */
export interface ContainerRef {
  id: string;
  name: string;
  project: string | null;
  service: string | null;
  working_dir: string | null;
  container_port: number | null;
}

/** "copy 2 of 2": which copy of its project a clone is, and the others' names. */
export interface CopyRef {
  rank: number;
  of: number;
  others: string[];
}

/** One listening port with everything the OS says about its holder. */
export interface Listener {
  key: string;
  proto: "tcp" | "udp";
  port: number;
  addresses: string[];
  scope: "loopback" | "all" | "lan";
  pid: number | null;
  name: string | null;
  exe: string | null;
  cwd: string | null;
  started_at: string | null;
  user: string | null;
  /** Started from a GitSwitch terminal. */
  ours: boolean;
  workspace: string | null;
  clone: CloneRef | null;
  copy: CopyRef | null;
  container: ContainerRef | null;
  killable: boolean;
  /** When not killable: why, in plain words. */
  why_not: string | null;
}

export interface PortsReport {
  os: "macos" | "linux" | "windows";
  rows: Listener[];
  /** What this listing could not see, one sentence each. */
  notes: string[];
  docker: "ok" | "missing" | "unavailable" | "timeout" | "skipped";
  took_ms: number;
}

export interface KillOutcome {
  pid: number;
  ended: boolean;
  how: "term" | "kill" | "taskkill" | "taskkill-force";
  group: boolean;
  message: string;
}

export interface ContainerStopOutcome {
  stopped: string[];
  message: string;
}

export async function portsList(): Promise<PortsReport> {
  return invoke("ports_list");
}

/** `name` is what the page listed: a pid reused by another program is refused. */
export async function portsKill(pid: number, force: boolean, name: string): Promise<KillOutcome> {
  return invoke("ports_kill", { pid, force, name });
}

export async function portsContainerStop(id: string, project: string | null): Promise<ContainerStopOutcome> {
  return invoke("ports_container_stop", { id, project });
}

/** One checkout of a project among the copies of it on this machine. */
export interface WorkspaceCopy {
  path: string;
  /** The folder name, with a parent prepended while two copies share one. */
  label: string;
  id: string;
  offset: number;
  kind: "clone" | "worktree";
  is_this: boolean;
  profile_name: string | null;
  branch: string | null;
  detached: boolean;
  unborn: boolean;
  /** Tracked files changed (untracked left out); null when unreadable. */
  changed: number | null;
}

export interface WorkspaceNote {
  level: "amber";
  text: string;
}

/**
 * What tells this checkout apart from another copy of the same project: its
 * id, port offset and Compose project name (the drawer exports them), the
 * other copies, and the note when two copies share a branch with changes.
 */
export interface WorkspaceInfo {
  /** The workspace's top folder (a submodule resolves to its superproject's). */
  path: string;
  name: string;
  id: string;
  kind: "clone" | "worktree";
  asked_path: string;
  offset: number;
  /** Set for every copy but the first; the first keeps Compose's own name. */
  compose_project: string | null;
  /** "owner/name" for GitHub, else the remote's address; null without a remote. */
  remote: string | null;
  /** Every copy, this one included; by offset, then path. */
  copies: WorkspaceCopy[];
  note: WorkspaceNote | null;
  /** What the drawer exports, as [name, value]. */
  env: [string, string][];
  banner: string;
  warnings: string[];
  /** The sentence about what is left to the person. */
  not_automated: string;
}

export async function workspaceInfo(path: string): Promise<WorkspaceInfo> {
  return invoke("workspace_info", { path });
}

/** The person chose this checkout's port offset (0..60000); one another copy uses is refused. */
export async function workspaceSetOffset(path: string, offset: number): Promise<WorkspaceInfo> {
  return invoke("workspace_set_offset", { path, offset });
}

export async function createProfile(params: {
  name: string;
  gitName: string;
  gitEmail: string;
  sshKeyPath?: string | null;
  directories: string[];
  allowPush?: boolean | null;
  signingEnabled?: boolean | null;
  remotePatterns?: string[] | null;
  githubLogin?: string | null;
  pushOverSsh?: boolean | null;
}): Promise<Profile> {
  return invoke("create_profile", params);
}

export async function updateProfile(params: {
  id: string;
  name?: string | null;
  gitName?: string | null;
  gitEmail?: string | null;
  sshKeyPath?: string | null;
  directories?: string[] | null;
  allowPush?: boolean | null;
  signingEnabled?: boolean | null;
  remotePatterns?: string[] | null;
  githubLogin?: string | null;
  pushOverSsh?: boolean | null;
}): Promise<Profile> {
  return invoke("update_profile", params);
}

export async function deleteProfile(id: string): Promise<void> {
  return invoke("delete_profile", { id });
}

export async function setDefaultProfile(id: string): Promise<Profile> {
  return invoke("set_default_profile", { id });
}

export async function generateSshKey(
  profileName: string,
  email: string
): Promise<[string, string]> {
  return invoke("generate_ssh_key", { profileName, email });
}

export async function getPublicKey(privateKeyPath: string): Promise<string> {
  return invoke("get_public_key", { privateKeyPath });
}

export async function testConnection(): Promise<string> {
  return invoke("test_connection");
}

export async function testConnectionWithKey(keyPath: string): Promise<string> {
  return invoke("test_connection_with_key", { keyPath });
}

export async function listSshKeys(): Promise<string[]> {
  return invoke("list_ssh_keys");
}

export async function deleteSshKey(keyPath: string): Promise<void> {
  return invoke("delete_ssh_key", { keyPath });
}

export async function ghListAccounts(): Promise<string[]> {
  return invoke("gh_list_accounts");
}

export async function ghGetToken(account: string): Promise<string> {
  return invoke("gh_get_token", { account });
}

export async function ghLogout(account: string): Promise<string> {
  return invoke("gh_logout", { account });
}

/** One login of the GitHub CLI: its host, and whether gh itself uses it (null from a gh that does not say). */
export interface GhAccount {
  host: string;
  login: string;
  active: boolean | null;
  scopes: string[] | null;
}

export async function ghAuthAccounts(): Promise<GhAccount[]> {
  return invoke("gh_auth_accounts");
}

/** `gh auth switch --hostname <host> --user <login>`: the sentence gh's listing confirms. */
export async function ghAuthSwitch(host: string, login: string): Promise<string> {
  return invoke("gh_auth_switch", { host, login });
}

/** The one-time code `gh auth login --web` shows, and the page to enter it on. */
export interface GhDevicePrompt {
  code: string;
  url: string;
}

/** Start gh's own browser sign-in (SSH as git's protocol, no key upload). */
export async function ghAuthLoginStart(host?: string): Promise<GhDevicePrompt> {
  return invoke("gh_auth_login_start", { host: host ?? null });
}

/** Wait for the sign-in started above: the account gh signed in. */
export async function ghAuthLoginWait(): Promise<{ host: string; login: string }> {
  return invoke("gh_auth_login_wait");
}

/** End the sign-in in progress, if any. */
export async function ghAuthLoginCancel(): Promise<void> {
  return invoke("gh_auth_login_cancel");
}

export async function ghRegisterSshKey(
  account: string,
  keyPath: string
): Promise<string> {
  return invoke("gh_register_ssh_key", { account, keyPath });
}

export async function resolveKeyAccount(
  keyPath: string
): Promise<string | null> {
  return invoke("resolve_key_account", { keyPath });
}

export interface RemoteChange {
  repo: string;
  old_url: string;
  new_url: string;
  changed: boolean;
  note: string;
}

export async function convertReposToSsh(
  directory: string
): Promise<RemoteChange[]> {
  return invoke("convert_repos_to_ssh", { directory });
}

export async function registerSigningKey(
  account: string,
  keyPath: string
): Promise<string> {
  return invoke("register_signing_key", { account, keyPath });
}

export async function signingKeyRegistered(
  account: string,
  keyPath: string
): Promise<boolean> {
  return invoke("signing_key_registered", { account, keyPath });
}

export async function allowedSignersPath(): Promise<string> {
  return invoke("allowed_signers_path");
}

export interface RepoRef {
  path: string;
  name: string;
  profile_id: string;
  profile_name: string;
  profile_email: string;
  /** A repository, or a profile folder that holds none (listed so it doesn't vanish). */
  kind?: "repo" | "folder";
  /** A folder entry inside a repository: that repository's top folder. */
  inside?: string | null;
}

/** What a folder is to git. */
export type FolderState =
  | { kind: "missing" }
  | { kind: "repo" }
  | { kind: "inside-repo"; top: string }
  | { kind: "contains-repos"; count: number }
  | { kind: "no-repo" };

/** What making a folder a repository made. */
export interface InitOutcome {
  path: string;
  branch: string;
}

export interface BranchInfo {
  name: string;
  is_current: boolean;
  is_remote: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  tip: string;
  short_tip: string;
  last_author: string;
  last_date: string;
  last_subject: string;
  /** One of GitSwitch's own backup branches (before an update, a reset, a rewrite). */
  is_backup?: boolean;
  /** The full ref: `refs/heads/x`, `refs/remotes/origin/x`. */
  full_ref?: string;
  /** A remote-tracking branch's name on its remote (`x` for `origin/x`); null for a local branch or an unknown remote. */
  remote_branch?: string | null;
}

export interface HistoryCommit {
  hash: string;
  short: string;
  parents: string[];
  author_name: string;
  author_email: string;
  date: string;
  subject: string;
  refs: string[];
  is_merge: boolean;
  /** The author date, stable across rebases; `date` is the committer date. */
  author_date: string;
  /** Submodule pointers this commit moves. */
  touches_submodules: string[];
  /** Changes nothing but submodule pointers: a bump, not work. */
  only_submodules: boolean;
  /** No remote-tracking branch has it: it exists only here (meaningful when the page has `remote_refs`). */
  unpushed: boolean;
  lane: number;
  parent_lanes: number[];
  active_lanes: number[];
  /** A lane was waiting for it: its line comes from above (false for a tip). */
  from_above?: boolean;
}

export interface HistoryPage {
  commits: HistoryCommit[];
  total: number;
  offset: number;
  limit: number;
  has_more: boolean;
  max_lane: number;
  /** At least one remote-tracking branch exists; without one nothing here is known to be pushed. */
  remote_refs: boolean;
  /** The comparison with the remote-tracking branches failed: no row is marked, and this says why. */
  marks_error?: string | null;
}

export interface FileChange {
  path: string;
  added: string;
  removed: string;
}

export interface CommitDetail {
  hash: string;
  short: string;
  author_name: string;
  author_email: string;
  author_date: string;
  committer_name: string;
  committer_email: string;
  subject: string;
  body: string;
  parents: string[];
  refs: string[];
  files: FileChange[];
  signature: string;
}

export interface CommitRef {
  short: string;
  subject: string;
  author: string;
  date: string;
}

export interface SyncStatus {
  branch: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  last_fetch_secs: number | null;
  incoming_rev: string | null;
  local_tip: CommitRef | null;
  remote_tip: CommitRef | null;
}

export interface FetchResult {
  message: string;
  head_unchanged: boolean;
  worktree_unchanged: boolean;
  local_branches_unchanged: boolean;
  updated_remote_refs: number;
}

export async function historyFetch(repoPath: string): Promise<FetchResult> {
  return invoke("history_fetch", { repoPath });
}

/** Who a push from this repository would authenticate as. */
export interface PushIdentity {
  remote: string | null;
  url: string | null;
  via: "ssh-key" | "ssh-agent" | "https-credential" | "blocked" | "local" | "none" | "unknown";
  login: string | null;
  key_path: string | null;
  host: string | null;
  account: string | null;
  expected_login: string | null;
  detail: string;
}

export async function changesPushIdentity(repoPath: string, refresh = false): Promise<PushIdentity> {
  return invoke("changes_push_identity", { repoPath, refresh });
}

/** One person who committed on the viewed revision, with how often. */
export interface AuthorCount {
  name: string;
  email: string;
  commits: number;
}

export async function historyAuthors(repoPath: string, rev: string): Promise<AuthorCount[]> {
  return invoke("history_authors", { repoPath, rev });
}

export interface HeadInfo {
  branch: string | null;
  detached: boolean;
  unborn: boolean;
}

/** Which branch HEAD is on, or that it is detached or unborn: three git reads
 *  where the full repository status is seventeen. */
export async function historyHead(repoPath: string): Promise<HeadInfo> {
  return invoke("history_head", { repoPath });
}

export async function historySyncStatus(
  repoPath: string,
  branch: string
): Promise<SyncStatus> {
  return invoke("history_sync_status", { repoPath, branch });
}

export interface MergeInfo {
  merged_into: string[];
  merges: HistoryCommit[];
  /** How the branch got into the first few branches that contain it. */
  merged_by?: MergedBy[];
  merges_total?: number;
  merges_truncated?: boolean;
}

export interface MergedBy {
  branch: string;
  /** merge | fast-forward | same-commit */
  how: string;
  commit: { hash: string; short: string; subject: string; author_name: string; date: string; parents: string[] } | null;
}

export async function historyListRepos(): Promise<RepoRef[]> {
  return invoke("history_list_repos");
}

/** What a folder is to git. Read only. */
export async function folderState(path: string): Promise<FolderState> {
  return invoke("folder_state", { path });
}

/** Make a folder that holds no repository one (`git init`). */
export async function repoInit(path: string): Promise<InitOutcome> {
  return invoke("repo_init", { path });
}

export async function historyBranches(repoPath: string): Promise<BranchInfo[]> {
  return invoke("history_branches", { repoPath });
}

export async function historyPage(
  repoPath: string,
  rev: string,
  offset: number,
  limit: number,
  search?: string | null,
  authors?: string[] | null,
  hidePointerOnly = false,
  unpushedOnly = false
): Promise<HistoryPage> {
  return invoke("history_page", {
    repoPath,
    rev,
    offset,
    limit,
    search: search ?? null,
    authors: authors ?? null,
    hidePointerOnly,
    unpushedOnly,
  });
}

export async function historyCommitDetail(
  repoPath: string,
  hash: string
): Promise<CommitDetail> {
  return invoke("history_commit_detail", { repoPath, hash });
}

export async function historyBranchMerges(
  repoPath: string,
  branch: string
): Promise<MergeInfo> {
  return invoke("history_branch_merges", { repoPath, branch });
}

export interface CommitInfo {
  hash: string;
  short: string;
  author_name: string;
  author_email: string;
  date: string;
  subject: string;
  pushed: boolean;
  /** The local branches holding it, when no remote-tracking branch does. */
  branches: string[];
  /** On the checked-out branch: what Fix rewrites. */
  on_head: boolean;
}

export interface RepoAudit {
  path: string;
  repo_name: string;
  profile_name: string;
  expected_name: string;
  expected_email: string;
  mismatched: CommitInfo[];
  unpushed_count: number;
  pushed_count: number;
  has_upstream: boolean;
  dirty: boolean;
  guard: "none" | "gitswitch" | "foreign";
  /** Wrong commits of yours on other local branches. */
  other_branch_count: number;
  /** Why Fix cannot run here now, when it cannot. */
  fix_blocker: string | null;
}

export async function auditCommits(): Promise<RepoAudit[]> {
  return invoke("audit_commits");
}

export async function fixUnpushedCommits(repoPath: string): Promise<string> {
  return invoke("fix_unpushed_commits", { repoPath });
}

export async function installCommitGuard(
  repoPath: string,
  expectedEmail: string
): Promise<string> {
  return invoke("install_commit_guard", { repoPath, expectedEmail });
}

export async function uninstallCommitGuard(repoPath: string): Promise<string> {
  return invoke("uninstall_commit_guard", { repoPath });
}

export interface Finding {
  id: string;
  severity: "error" | "warning" | "info" | "ok";
  title: string;
  detail: string;
  fix: string | null;
  fix_label: string | null;
}

export async function doctorCheckEnvironment(): Promise<Finding[]> {
  return invoke("doctor_check_environment");
}

export async function doctorCheckProfiles(): Promise<Finding[]> {
  return invoke("doctor_check_profiles");
}

export async function doctorCheckKeys(): Promise<Finding[]> {
  return invoke("doctor_check_keys");
}

export async function doctorCheckRepos(): Promise<Finding[]> {
  return invoke("doctor_check_repos");
}

/** Doctor's repair for a repository-local identity override. */
/** Doctor, on request: can the account each repository pushes as see it on GitHub? */
export async function doctorCheckRepoAccess(): Promise<Finding[]> {
  return invoke("doctor_check_repo_access");
}

/** One of Doctor's own repairs, by its finding's fix id. */
export async function doctorApplyFix(fix: string): Promise<string> {
  return invoke("doctor_apply_fix", { fix });
}

export async function doctorUnsetLocal(repoPath: string, key: string): Promise<string> {
  return invoke("doctor_unset_local", { repoPath, key });
}

export async function doctorFixSshConfig(): Promise<string> {
  return invoke("doctor_fix_ssh_config");
}

export interface ScannedRepo {
  path: string;
  remote_url: string;
  /** origin's own push URL, when set and different from remote_url (masked). */
  push_url?: string;
  owner: string;
  name: string;
}

export interface RepoPermissions {
  admin: boolean;
  push: boolean;
  pull: boolean;
}

export async function scanRepos(root: string): Promise<ScannedRepo[]> {
  return invoke("scan_repos", { root });
}

/** A folder scan the page can watch (findReposProgress) and stop (findReposCancel) by its id. */
export interface TrackedScan {
  root: string;
  repos: ScannedRepo[];
  folders: number;
  /** Stopped before it ended: repos is then empty. */
  cancelled: boolean;
}

export async function scanReposTracked(scanId: string, root: string): Promise<TrackedScan> {
  return invoke("scan_repos_tracked", { scanId, root });
}

export async function checkRepoAccess(
  token: string,
  owner: string,
  repo: string
): Promise<RepoPermissions | null> {
  return invoke("check_repo_access", { token, owner, repo });
}

// --- Find repositories: clones no profile folder holds ------------------------

/** Where a repository's commits get their email from today. */
export interface CommitsAs {
  profile_id: string | null;
  /**
   * folder: a profile folder holds it, deeper than the page looks · rule: a
   * profile's remote rule · gitdir: a worktree (or separate git folder), which
   * follows where its git folder is · default: the default profile · local:
   * the repository sets its own email · none: no email at all.
   */
  via: "folder" | "rule" | "gitdir" | "default" | "local" | "none";
}

/** A repository the Repository page does not list. */
export interface FoundRepo extends ScannedRepo {
  /** The GitHub repository pushes go to (the push URL's, else the origin's); empty when that is not GitHub. */
  push_owner: string;
  push_name: string;
  /** `.git` is a file: a worktree or a separate git folder. */
  gitdir_elsewhere: boolean;
  commits_as: CommitsAs;
}

/** A repository found that a profile's folder already holds. */
export interface ListedRepo {
  path: string;
  profile_id: string;
  profile_name: string;
  /** The GitHub account the profile names, when it names one. */
  github_login: string | null;
}

export interface FoundClones {
  /** The home folder, for showing paths under it as `~/…`. */
  home: string | null;
  /** The places searched, in order. */
  roots: string[];
  repos: FoundRepo[];
  /** Repositories found that the Repository page lists already. */
  already_listed: number;
  /** Those repositories, each with the profile the Repository page shows it under. */
  listed?: ListedRepo[];
  truncated: { place: string; why: string }[];
  /** Folders at the top of a place that could not be read (a macOS privacy refusal, say). */
  unreadable: string[];
  rejected: { place: string; why: string }[];
  /** Linked folders near the home folder that lead somewhere not searched. */
  linked: { from: string; to: string }[];
  cancelled: boolean;
}

export interface FindProgress {
  /** "looking" while folders are walked, "checking" while GitHub is asked. */
  phase: string;
  folders: number;
  found: number;
  checked: number;
  total: number;
  place: string;
}

/** One account and one repository. */
export interface AccessCell {
  state: "visible" | "invisible" | "unknown";
  push: boolean;
  admin: boolean;
  /** Only the accounts it was shared with can read it. */
  private: boolean | null;
  why: string | null;
}

export interface AccessMatrix {
  /** Every account asked, as GitHub spells its login. */
  accounts: { login: string; source: string }[];
  /** Accounts whose token could not be read or is not accepted. */
  problems: { account: string; why: string }[];
  /** "owner/name" (lowercase) -> login (lowercase) -> the answer. */
  access: Record<string, Record<string, AccessCell>>;
  cancelled: boolean;
}

export interface FolderChange {
  profile_id: string;
  path: string;
}

export interface FolderAdded extends FolderChange {
  /** The email the repository's identity guard expected before, when it has one. */
  guard_was: string | null;
}

export interface FoldersAdded {
  added: FolderAdded[];
  failed: (FolderChange & { why: string })[];
}

export async function findUnlistedRepos(scanId: string, extraRoots: string[]): Promise<FoundClones> {
  return invoke("find_unlisted_repos", { scanId, extraRoots });
}

export async function findReposProgress(scanId: string): Promise<FindProgress | null> {
  return invoke("find_repos_progress", { scanId });
}

export async function findReposCancel(scanId: string): Promise<void> {
  return invoke("find_repos_cancel", { scanId });
}

/** What every signed-in account (GitHub CLI and GitSwitch sign-ins) may do with each "owner/name". */
export async function repoAccessMatrix(scanId: string, repos: string[]): Promise<AccessMatrix> {
  return invoke("repo_access_matrix", { scanId, repos });
}

export async function profileFoldersAdd(changes: FolderChange[]): Promise<FoldersAdded> {
  return invoke("profile_folders_add", { changes });
}

/** Undo of profileFoldersAdd: hand back what it returned. */
export async function profileFoldersRemove(changes: FolderAdded[]): Promise<FolderChange[]> {
  return invoke("profile_folders_remove", { changes });
}

export interface SparseInfo {
  path: string;
  branch: string;
  is_sparse: boolean;
  sparse_dirs: string[];
  available_dirs: string[];
}

export interface SubmoduleReport {
  /** submodules with an address in .gitmodules */
  listed: number;
  /** of those, actually checked out */
  downloaded: number;
  /** entries with no address — git can't fetch them */
  unlisted: number;
  /** the listed submodules whose update failed, by path */
  failed_paths: string[];
  error: string | null;
  /** Submodules whose interrupted download was cleared before downloading again. */
  cleared?: string[];
}

/** The large files after a checkout — present whenever the repo uses Git LFS. */
export interface LfsReport {
  installed: boolean;
  requested: boolean;
  tracked: number;
  fetched: number;
  pointers_left: number;
  configured_now: boolean;
  error: string | null;
  /** One sentence for the UI, written by the backend. */
  note: string;
}

export interface LfsTool {
  installed: boolean;
  version: string | null;
  install_hint: string;
}

export interface CloneResult {
  path: string;
  /** Profile whose SSH key authenticated the clone. */
  profile_name: string | null;
  /** Set when the new folder was added to that profile. */
  mapped_to: string | null;
  submodules: SubmoduleReport | null;
  lfs: LfsReport | null;
  /** The same for each populated submodule that uses Git LFS. */
  submodule_lfs?: { path: string; report: LfsReport }[];
}

export interface SparseSetResult {
  lfs: LfsReport | null;
}

export async function lfsAvailable(): Promise<LfsTool> {
  return invoke("lfs_available");
}

/** `cloneAs` = profile id to authenticate as; null lets the folder decide. */
/** One line of `git clone --progress`, while a clone runs. */
export interface CloneProgress {
  phase: string;
  percent: number | null;
  done: number | null;
  total: number | null;
  bytes: string | null;
  rate: string | null;
  /** What is being worked on right now: the submodule being fetched, the large file being downloaded. */
  detail?: string;
  /** What the host says the repository being downloaded weighs, in kilobytes; absent when it does not say. */
  estimate_kb?: number;
  /** Seconds left at the current rate; absent when there is nothing to base it on yet. */
  eta_secs?: number;
}

export async function cloneProgress(destPath: string): Promise<CloneProgress | null> {
  return invoke("clone_progress", { destPath });
}

/** GitHub's size for a repository (kilobytes of git data; LFS content is extra). */
export interface CloneSizeEstimate {
  repo: string;
  kb: number;
}

export async function cloneSizeEstimate(url: string): Promise<CloneSizeEstimate | null> {
  return invoke("clone_size_estimate", { url });
}

export async function sparseClone(
  url: string,
  parentDir: string,
  folderName?: string | null,
  cloneAs?: string | null
): Promise<CloneResult> {
  return invoke("sparse_clone", {
    url,
    parentDir,
    folderName: folderName ?? null,
    cloneAs: cloneAs || null,
  });
}

export async function fullClone(
  url: string,
  parentDir: string,
  folderName?: string | null,
  cloneAs?: string | null,
  withSubmodules = true,
  withLfs = true
): Promise<CloneResult> {
  return invoke("full_clone", {
    url,
    parentDir,
    folderName: folderName ?? null,
    cloneAs: cloneAs || null,
    withSubmodules,
    withLfs,
  });
}

export interface RepoAccount {
  /** "gh:<login>" (a GitHub CLI login) or "signin:<login>" (a sign-in kept here) */
  key: string;
  label: string;
  source: string;
}

export interface RemoteRepo {
  full_name: string;
  owner: string;
  name: string;
  owner_is_org: boolean;
  description: string | null;
  private: boolean;
  archived: boolean;
  fork: boolean;
  default_branch: string;
  clone_url: string;
  pushed_at: string | null;
  permission: "admin" | "write" | "read";
  local_path: string | null;
  suggested_profile_id: string | null;
}

export interface RepoListing {
  account: string;
  login: string;
  suggested_profile_id: string | null;
  repos: RemoteRepo[];
  truncated: boolean;
  sso_hidden_orgs: number;
}

export interface LocalClones {
  /** "owner/name" (lowercase) -> local path */
  clones: Record<string, string>;
  /** owner (lowercase) -> profile id its local repos belong to */
  owner_profiles: Record<string, string>;
  /** owner (lowercase) -> "org-<id>" SSH user seen locally */
  org_ssh_users: Record<string, string>;
  /** "owner/name" (lowercase) -> the folder a clone is still writing it into */
  cloning?: Record<string, string>;
  /** "owner/name" (lowercase) -> every local copy, for repositories cloned more than once */
  copies?: Record<string, string[]>;
}

/** Which repos are on this machine right now — local disk only, no network. */
export async function localCloneIndex(): Promise<LocalClones> {
  return invoke("local_clone_index");
}

/** Does this path still exist? */
export async function pathExists(path: string): Promise<boolean> {
  return invoke("path_exists", { path });
}

/** GitHub accounts that can list repositories (GitHub CLI + GitSwitch sign-ins). */
export async function repoAccounts(): Promise<RepoAccount[]> {
  return invoke("repo_accounts");
}

/** Every repository the account can reach (all pages). */
export async function listRemoteRepos(account: string): Promise<RepoListing> {
  return invoke("list_remote_repos", { account });
}

/** One page of the listing; `listing.repos` is that page only, `login` is filled on page 1. */
export interface RepoPage {
  listing: RepoListing;
  page: number;
  per_page: number;
  has_more: boolean;
}

export async function listRemoteReposPage(account: string, page: number, perPage: number): Promise<RepoPage> {
  return invoke("list_remote_repos_page", { account, page, perPage });
}

export interface CertInfo {
  path: string;
  exists: boolean;
  valid_until: string | null;
  expired: boolean;
}

export interface DestinationStatus {
  path: string;
  exists: boolean;
  is_repo: boolean;
  origin: string | null;
  same_repo: boolean;
  same_url: boolean;
  origin_is_https: boolean;
  /** repo folder with no commit — an interrupted clone (or an empty repo) */
  incomplete: boolean;
}

/** Is the clone destination already taken — and by this same repo? (local only) */
export async function cloneDestinationStatus(
  url: string,
  parentDir: string,
  folderName?: string | null
): Promise<DestinationStatus> {
  return invoke("clone_destination_status", { url, parentDir, folderName: folderName || null });
}

/** Point an existing clone's origin at another link to the SAME repo. */
export async function switchRepoOrigin(repoPath: string, url: string): Promise<string> {
  return invoke("switch_repo_origin", { repoPath, url });
}

/** Company-signed SSH certificate next to a key (`<key>-cert.pub`), if any. */
export async function sshCertificateInfo(keyPath: string): Promise<CertInfo> {
  return invoke("ssh_certificate_info", { keyPath });
}

export async function sparseRepoInfo(repoPath: string): Promise<SparseInfo> {
  return invoke("sparse_repo_info", { repoPath });
}

export async function sparseSet(
  repoPath: string,
  dirs: string[],
  withLfs = true
): Promise<SparseSetResult> {
  return invoke("sparse_set", { repoPath, dirs, withLfs });
}

export async function sparseAdd(
  repoPath: string,
  dirs: string[]
): Promise<void> {
  return invoke("sparse_add", { repoPath, dirs });
}

/** A login signed in with "Sign in with GitHub" and kept here; its token is in the system's credential store. */
export interface KeptSignIn {
  login: string;
  id: number;
}

/** Keep a "Sign in with GitHub": GitHub is asked whose token it is, and the token goes to the system's credential store. */
export async function githubSigninKeep(token: string): Promise<GitHubUser> {
  return invoke("github_signin_keep", { token });
}

/** Forget a kept sign-in: its token and its login. */
export async function githubSigninForget(login: string): Promise<void> {
  return invoke("github_signin_forget", { login });
}

/** The sign-ins kept here, newest first. */
export async function githubSignins(): Promise<KeptSignIn[]> {
  return invoke("github_signins");
}

export async function applyGitConfig(): Promise<void> {
  return invoke("apply_git_config");
}

export async function getCurrentGitConfig(): Promise<string> {
  return invoke("get_current_git_config");
}

export interface GitHubUser {
  id: number;
  login: string;
  name: string | null;
  /** The public address, if the account shows one. */
  email: string | null;
  avatar_url: string;
  /** The account's verified addresses, primary first (needs the user:email scope). */
  emails?: string[];
  primary_email?: string | null;
  /** The token could not read the account's addresses. */
  email_scope_missing?: boolean;
}

export interface GitHubKey {
  id: number;
  title: string;
  key: string;
}

export async function verifyGithubToken(token: string): Promise<GitHubUser> {
  return invoke("verify_github_token", { token });
}

export async function uploadSshKeyToGithub(
  token: string,
  title: string,
  publicKey: string
): Promise<GitHubKey> {
  return invoke("upload_ssh_key_to_github", { token, title, publicKey });
}

export async function listGithubSshKeys(token: string): Promise<GitHubKey[]> {
  return invoke("list_github_ssh_keys", { token });
}

export interface DeviceCodeResponse {
  device_code: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

export interface OAuthTokenResponse {
  access_token: string;
  token_type: string;
  scope: string;
}

export async function githubDeviceCode(
  clientId?: string
): Promise<DeviceCodeResponse> {
  return invoke("github_device_code", { clientId: clientId ?? null });
}

export async function githubPollToken(
  deviceCode: string,
  clientId?: string
): Promise<OAuthTokenResponse> {
  return invoke("github_poll_token", {
    deviceCode,
    clientId: clientId ?? null,
  });
}

// --- Changes page: working-tree state, diffs, and git operations ---

export interface ChangeEntry {
  path: string;
  orig_path: string | null;
  /** Index vs HEAD: "M" "T" "A" "D" "R" "C" or "." */
  staged: string;
  /** Worktree vs index: "M" "T" "D" "?" or "." */
  unstaged: string;
  kind: "tracked" | "untracked" | "conflicted";
  conflict: string | null;
  rename_score: number | null;
  is_submodule: boolean;
  sub_commit_changed: boolean;
  sub_tracked_changes: boolean;
  sub_untracked: boolean;
  /** For a submodule, the commit the superproject records. */
  recorded_oid: string | null;
  /** Conflicted entries only: modes and oids of index stages 1, 2, 3 (base, ours, theirs). */
  stage_modes?: string[];
  stage_oids?: string[];
  /** A conflicted submodule pointer: the commit Keep mine / Take theirs records (sides already mapped). */
  pointer_mine?: string;
  pointer_theirs?: string;
  staged_added: number | null;
  staged_removed: number | null;
  unstaged_added: number | null;
  unstaged_removed: number | null;
  is_binary: boolean;
}

/** The two sides of a conflict, named the way the user thinks of them. */
export interface Sides {
  mine: string;
  theirs: string;
}

export interface InProgress {
  kind: string;
  label: string;
  detail: string;
  abort_command: string;
  /** What finishes it once conflicts are staged; null when only a commit or nothing can. */
  continue_command: string | null;
  sides?: Sides | null;
}

/** A sync that is paused or running in this repository (or in its superproject). */
export interface SyncSummary {
  /** preparing | rebasing | aligning | done | aborted */
  phase: string;
  started_at: string;
  needs_user: SyncNeedsUser | null;
  /** The superproject the sync belongs to — this repo, or the parent of a submodule. */
  root: string;
  /** False when this repository is a submodule of the one being synced. */
  is_root: boolean;
  /** Conflicted files still unmerged where the pause is: 0 means Continue update can be pressed. */
  unresolved: number;
  /** The superproject's upstream, what "theirs" means in every rebase the update runs. */
  upstream: string;
  /** For a submodule's own status: its path in the superproject and the commit it is rebased onto. */
  sub_path: string | null;
  sub_target: string | null;
  /**
   * The pause is a submodule pointer the update could not place itself, still
   * unmerged here: stage it in this repository ("Mark resolved" on its row).
   */
  handed_over?: boolean;
}

export interface SyncNeedsUser {
  /** The commit of yours being replayed when the superproject's rebase stopped. */
  at_commit?: string | null;
  /** superproject | submodule:<path> | submodule-autostash:<path> | autostash-conflict */
  where_: string;
  paths: string[];
  hint: string;
}

export interface RepoIdentity {
  name: string;
  email: string;
  email_scope: string;
  email_origin: string | null;
  profile_id: string | null;
  profile_name: string | null;
  profile_email: string | null;
  matches_profile: boolean;
  signing_on: boolean;
  signing_key: string | null;
  guard: "gitswitch" | "foreign" | "none";
  /** The email GitSwitch's guard expects; it goes stale when the profile's email changes. */
  guard_email: string | null;
}

export interface RemotePushState {
  name: string;
  fetch_url: string;
  push_url: string;
  rewritten: boolean;
  has_explicit_pushurl: boolean;
}

export interface LockEvent {
  ts: string;
  repo: string;
  code: string;
  detail: string;
  healed: boolean;
}

/** The tamper-resistant lock, measured. Nested in PushState because the page replaces `status.push` wholesale. */
export interface LockState {
  supported: boolean;
  platform: string | null;
  locked: boolean;
  registry: "ok" | "missing" | "untrusted" | "unreadable" | string;
  system_include: "ok" | "missing" | "wrong-target" | string;
  stanza: "ok" | "missing" | "stale" | string;
  system_rewrite_measured: boolean;
  helper: "ok" | "missing" | "outdated" | "untrusted" | string;
  helper_installed: boolean;
  remote_helper: "ok" | "missing" | "foreign" | "unprotected-dir" | string;
  mirrors: "ok" | "healed" | "drifted" | "unfixable" | string;
  drift: string[];
  needs_elevation: boolean;
  caveats: string[];
  locked_at: string | null;
  recent_events: LockEvent[];
  system_gitconfig: string | null;
  audit_log: string | null;
  bundled_helper_sha256: string | null;
}

export interface PushState {
  profile_blocked: boolean;
  profile_name: string | null;
  repo_blocked: boolean;
  blocked: boolean;
  reason: string;
  hook: "gitswitch" | "chained" | "foreign" | "none";
  hooks_path_overridden: string | null;
  config_block_present: boolean;
  remotes: RemotePushState[];
  gaps: string[];
  needs_repair: boolean;
  lock: LockState;
}

export type PushMode = "allowed" | "guardrail" | "locked";

/** One submodule's share of the repository's push block or lock. */
export interface SubCoverage {
  path: string;
  blocked: boolean;
  locked: boolean;
  needs_repair: boolean;
  /** It has what the repository has: a lock under a locked repository, at least the guard-rail under a blocked one. */
  covered: boolean;
  /** When not covered: what it has instead, in plain words. */
  why: string;
  /** Its block is the repository's doing: the repository's "Allowed" takes it away (with the prompt when it is a lock). */
  by_superproject?: boolean;
  /** When its lock is not whole: what was measured, in the lock's own terms. */
  drift?: string[];
}

/** How far the repository's push block or lock reaches into its submodules — where the code lives. */
export interface PushCoverage {
  mode: PushMode;
  submodules: SubCoverage[];
  /** Submodules with less than the repository has: a push from inside them is not stopped. */
  uncovered: string[];
  /** Repair can do something about what is uncovered. */
  needs_repair: boolean;
  /** ...and part of it is the administrator's: Repair shows the prompt. */
  needs_elevation?: boolean;
  /** One sentence for the card; empty when there is nothing to say. */
  summary: string;
}

export type JobOutcomeKind =
  | "applied"
  /** What could be done was done; something is still not covered (the message says what). */
  | "incomplete"
  | "cancelled"
  | "denied"
  | "no-agent"
  | "manual-required"
  | "busy"
  | "failed"
  | "refused"
  | "unsupported";

export interface LockChange {
  op: string;
  repo?: string | null;
  detail: string;
}

export interface JobOutcome {
  outcome: JobOutcomeKind;
  message: string;
  command: string | null;
  job_nonce: string | null;
  changed: LockChange[];
  errors: string[];
  bootstrapped: boolean;
}

export interface PushModeResult {
  outcome: JobOutcomeKind;
  message: string;
  command: string | null;
  job_nonce: string | null;
  state: PushState | null;
}

export interface LockSummary {
  repo: string;
  label: string;
  locked_at: string;
  exists: boolean;
}

export interface HelperStatus {
  supported: boolean;
  platform: string | null;
  helper_path: string | null;
  installed: boolean;
  installed_version: string | null;
  installed_sha256: string | null;
  helper: "ok" | "missing" | "outdated" | "untrusted" | string;
  bundled_path: string | null;
  bundled_version: string | null;
  bundled_sha256: string | null;
  bundled_signed: boolean;
  registry: string;
  locks: LockSummary[];
  system_gitconfig: string | null;
  remote_helper: string;
  remote_helper_path: string | null;
  registry_dir: string | null;
  audit_log: string | null;
  events_log: string | null;
  uac: { enabled: boolean; admin_behavior: number } | null;
}

export interface RepoStatus {
  path: string;
  name: string;
  branch: string | null;
  detached: boolean;
  unborn: boolean;
  head_oid: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  last_fetch_secs: number | null;
  entries: ChangeEntry[];
  staged_count: number;
  unstaged_count: number;
  untracked_count: number;
  conflicted_count: number;
  submodule_dirty_count: number;
  untracked_truncated: boolean;
  stash_count: number;
  operation: InProgress | null;
  identity: RepoIdentity;
  push: PushState;
  can_amend: boolean;
  head_subject: string | null;
  merge_message: string | null;
  has_submodules: boolean;
  /** Some tracked .gitattributes routes files through Git LFS. */
  uses_lfs: boolean;
  /** A paused sync here or in the superproject this repo belongs to. */
  sync?: SyncSummary | null;
  /** Conflicts with no operation in progress: "stash" or "autostash" re-apply left them. */
  conflict_source?: string | null;
}

export interface MovedCommit {
  short: string;
  subject: string;
}

/** One gitlink, with everything that can be known about it locally. */
export interface SubmoduleInfo {
  path: string;
  name: string | null;
  url: string | null;
  configured_branch: string | null;
  recorded: string;
  recorded_short: string;
  actual: string | null;
  actual_short: string | null;
  initialised: boolean;
  /** Has a .gitmodules entry. Without one, git has no address to fetch from. */
  listed: boolean;
  ahead: number;
  behind: number;
  recorded_missing: boolean;
  moved_commits: MovedCommit[];
  more_moved: number;
  own_branch: string | null;
  own_upstream: string | null;
  own_ahead: number;
  own_behind: number;
  dirty_tracked: number;
  dirty_untracked: number;
  /** The untracked count hit the parser's cap — treat it as a floor. */
  dirty_untracked_capped: boolean;
  state:
    | "clean"
    | "moved"
    | "dirty"
    | "moved-and-dirty"
    | "not-initialised"
    | "unmapped"
    /** A `.git` entry but nothing checked out: a download that was interrupted. Downloadable again. */
    | "incomplete";
  summary: string;
  gitdir_valid: boolean;
  operation: string | null;
  conflicted: number;
  upstream_remote: string | null;
  /**
   * The recorded commit is on one of the submodule's remote-tracking branches,
   * so a clone of this repository can get it. `null` when that cannot be known
   * here (not initialised, the commit missing, or no remote-tracking branch).
   */
  recorded_on_remote: boolean | null;
  /** Commits at its checkout that no remote-tracking branch has; `null` without one to compare with. */
  own_unpushed: number | null;
  /** `<remote>/<branch>` a clone would follow, when that remote-tracking branch is here. */
  latest_ref?: string | null;
}

export interface LfsStatus {
  installed: boolean;
  version: string | null;
  uses_lfs: boolean;
  /** `git lfs install --local` has been run here (or globally). */
  filters_configured: boolean;
  tracked: number;
  /** Files still holding a pointer stub — what `git lfs pull` would fetch. */
  pointers: number;
  pointer_paths: string[];
  more_pointers: number;
  summary: string;
  /** How to install git-lfs on this platform, for when it is missing. */
  install_hint: string;
}

export async function changesLfsStatus(repoPath: string): Promise<LfsStatus> {
  return invoke("changes_lfs_status", { repoPath });
}

export async function changesLfsPull(repoPath: string): Promise<OpResult> {
  return invoke("changes_lfs_pull", { repoPath });
}

/** One LFS-tracked file in the checkout. */
export interface LfsFile {
  path: string;
  /** The folder holding it, "" at the repository root. */
  dir: string;
  /** What the real content weighs, known even while the file is a stub. */
  size: number;
  /** The working file holds the real content. */
  present: boolean;
  /** The object is already in this clone, so it only needs writing out. */
  downloaded: boolean;
}

/** A folder's totals, including everything nested beneath it. */
export interface LfsFolder {
  path: string;
  depth: number;
  files: number;
  missing: number;
  bytes: number;
  missing_bytes: number;
}

export interface LfsListing {
  installed: boolean;
  version: string | null;
  uses_lfs: boolean;
  filters_configured: boolean;
  files: LfsFile[];
  folders: LfsFolder[];
  total: number;
  present: number;
  missing: number;
  total_bytes: number;
  missing_bytes: number;
  /** False on an older git-lfs that reports no sizes — don't print "0 B". */
  sizes_known: boolean;
  /** Files past the cap that were not sent; folder totals still count them. */
  truncated: number;
  folders_truncated: number;
  summary: string;
}

/** How far a running download has got. */
export interface LfsProgress {
  file: string;
  done: number;
  total: number;
  bytes: number;
  total_bytes: number;
}

export async function changesLfsFiles(repoPath: string): Promise<LfsListing> {
  return invoke("changes_lfs_files", { repoPath });
}

/** Download only these files and folders. */
export async function changesLfsPullPaths(repoPath: string, paths: string[]): Promise<OpResult> {
  return invoke("changes_lfs_pull_paths", { repoPath, paths });
}

export async function changesLfsProgress(repoPath: string): Promise<LfsProgress | null> {
  return invoke("changes_lfs_progress", { repoPath });
}

export async function changesSubmodules(repoPath: string): Promise<SubmoduleInfo[]> {
  return invoke("changes_submodules", { repoPath });
}

export interface DiffLine {
  kind: "hunk" | "add" | "del" | "context" | "meta";
  text: string;
}

export interface FileDiff {
  path: string;
  staged: boolean;
  lines: DiffLine[];
  is_binary: boolean;
  truncated: boolean;
  total_lines: number;
  added: number;
  removed: number;
  empty_reason: string | null;
}

export interface Advice {
  headline: string;
  guidance: string;
  action: string | null;
  /** The page the action's button opens (GitHub's new-repository form, filled in). */
  link?: string | null;
  /** The remote the action acts on. */
  remote?: string | null;
  git_said: string;
}

export interface Refusal {
  code: string;
  message: string;
}

export interface CommitResult {
  hash: string;
  short: string;
  subject: string;
  author_name: string;
  author_email: string;
  signature: string;
  signed: boolean;
  amended: boolean;
  files_changed: number;
  insertions: number;
  deletions: number;
}

export interface PushedRef {
  flag: string;
  summary: string;
  from: string;
  to: string;
}

export interface PushOutcome {
  refs: PushedRef[];
  up_to_date: boolean;
  new_upstream: string | null;
  remote: string;
}

export interface PullOutcome {
  mode: string;
  head_before: string;
  head_after: string;
  fast_forward: boolean;
  commits_pulled: number;
  files_changed: number;
  conflicts: string[];
  recovery: string | null;
  /** A rebase with autostash finished but the stash could not be re-applied cleanly. */
  autostash_conflict?: boolean;
  autostash_left?: string | null;
  /** Submodule pointers the pull moved: the repository now records other commits than are checked out. */
  submodules_moved?: number;
}

/** What every git operation returns: the outcome plus the re-read repo state. */
export interface OpResult {
  ok: boolean;
  headline: string;
  detail: string;
  advice?: Advice;
  refusal?: Refusal;
  status?: RepoStatus;
  commit?: CommitResult;
  push?: PushOutcome;
  pull?: PullOutcome;
  submodules?: SubmoduleReport;
  lfs?: LfsStatus;
  sync?: SyncOutcome;
  tree?: TreeOutcome;
  stash?: StashOutcome;
  state?: StateOutcome;
  /** Submodules a push would leave a clone unable to check out, or could not check. */
  unpushed_submodules?: SubmodulePush[];
  /**
   * Set by the page on a result that belongs to a submodule it ran an operation
   * in (its path), not to the repository on screen: actions keyed on the
   * result's codes are not offered for it.
   */
  inside?: string;
}

/**
 * A submodule whose recorded commit a push advertises while none of the
 * submodule's remotes has it (`not-on-remote`), or where that could not be
 * checked (`not-here`: the commit is not in its checkout; `not-populated`).
 */
export interface SubmodulePush {
  path: string;
  recorded: string;
  recorded_short: string;
  state: "not-on-remote" | "not-here" | "not-populated";
  /** Its checked-out branch; null when detached, where a push from it needs a branch first. */
  branch: string | null;
}

// --- Tree management: branches, undo, reset, revert, cherry-pick, conflicts ----

export interface TreeCommitRef {
  oid: string;
  short: string;
  subject: string;
}

export interface StashRef {
  ref: string;
  oid: string;
  message: string;
}

export interface TreeOutcome {
  /** switch | create-branch | delete-branch | rename-branch | undo-commit | reset | detach | revert | cherry-pick | resolve-side | discard-all | align-submodules */
  op: string;
  head_before: string | null;
  head_after: string | null;
  branch_before: string | null;
  branch_after: string | null;
  commit: TreeCommitRef | null;
  parents: TreeCommitRef[];
  dropped: TreeCommitRef[];
  dropped_total: number;
  backup: SyncBackupRef | null;
  stash: StashRef | null;
  recovery: string | null;
  submodule_mismatch: string[];
  blocking_files: string[];
  conflicts: string[];
  mapping_note: string | null;
  deleted_untracked: number;
  restored_tracked: number;
}

/** A commit the user typed or picked, with what a reset to it would do. */
export interface CommitTarget {
  input: string;
  oid: string;
  short: string;
  subject: string;
  author: string;
  date: string;
  is_merge: boolean;
  parents: TreeCommitRef[];
  is_head: boolean;
  contained_in_head: boolean;
  dropped_if_reset: number;
  would_drop_pushed: boolean;
  on_remote: boolean;
}

export type ResetMode = "soft" | "mixed" | "hard";
export type ConflictSide = "mine" | "theirs";

/** One submodule's full status, for its own section on the Changes page. */
export interface SubmoduleStatus {
  /** Root-relative, byte-identical to the parent's gitlink row. */
  path: string;
  name: string | null;
  listed: boolean;
  recorded: string;
  recorded_short: string;
  status: RepoStatus;
}

export interface StashEntry {
  index: number;
  ref: string;
  oid: string;
  message: string;
  branch: string | null;
  date: string;
  tracked_files: number;
  untracked_files: number;
  is_autostash: boolean;
}

export interface StashFile {
  path: string;
  /** M | A | D | R | untracked */
  status: string;
  added: number | null;
  removed: number | null;
}

export interface StashDetail {
  entry: StashEntry;
  files: StashFile[];
}

export interface StashOutcome {
  /** push | apply | pop | drop | restore */
  action: string;
  entry: StashRef | null;
  stash_count: number;
  conflicts: string[];
  kept: boolean;
  recovery: string | null;
  not_stashed_submodules: string[];
}

/** A repository inside a saved state (a submodule or a nested repository). */
export interface StateSub {
  path: string;
  /** Checked out now: its part can come back. */
  downloaded: boolean;
  changed: number;
  branch: string | null;
  /** Commits on its saved branch since that are already pushed (that branch is kept). */
  pushed_since: number;
}

/** A saved state: where the repository was — commit, branch, staged and every changed file. */
export interface RepoState {
  /** "1".."4" or "before". */
  slot: string;
  oid: string;
  name: string;
  /** null: HEAD was detached. */
  branch: string | null;
  head: string;
  head_short: string;
  head_subject: string;
  /** Seconds since 1970. */
  saved: number;
  changed: number;
  staged: number;
  new_files: number;
  branch_gone: boolean;
  /** Commits the branch gained since (they leave it on Go back, kept on a backup branch). */
  commits_since: number;
  /** Of those, already pushed. */
  pushed_since: number;
  upstream: string | null;
  /** For "Before going back": the state that was gone back to. */
  before_of: string | null;
  subs: StateSub[];
}

export interface StatesList {
  /** The saved states, by slot. */
  states: RepoState[];
  before: RepoState | null;
  /** How many this repository keeps (its own setting; 4 until changed). */
  max: number;
  /** The slot a new state goes into; null when every slot is in use. */
  next_free: string | null;
  /** The most it can be set to. */
  limit: number;
}

export interface StateOutcome {
  /** save | restore | delete */
  action: string;
  slot: string;
  name: string;
  oid: string | null;
  branch_before: string | null;
  branch_after: string | null;
  head_before: string | null;
  head_after: string | null;
  before_saved: string | null;
  new_branch: string | null;
  backup: string | null;
  recovery: string | null;
  notes: string[];
}

/** The four state slots and the automatic "Before going back". */
export async function statesList(repoPath: string): Promise<StatesList> {
  return invoke("states_list", { repoPath });
}

/** Save where the repository is now into a slot; `replace`: the state shown there, to replace it. */
export async function statesSave(repoPath: string, slot: string, name: string | null, replace: string | null): Promise<OpResult> {
  return invoke("states_save", { repoPath, slot, name, replace });
}

/** Go back to a state (named by the oid shown). `newBranch`: on a new branch; `allowPushed`: past pushed commits anyway. */
export async function statesRestore(repoPath: string, slot: string, oid: string, newBranch: string | null, allowPushed: boolean): Promise<OpResult> {
  return invoke("states_restore", { repoPath, slot, oid, newBranch, allowPushed });
}

export async function statesDelete(repoPath: string, slot: string, oid: string): Promise<OpResult> {
  return invoke("states_delete", { repoPath, slot, oid });
}

/** Keep up to `max` states in this repository; refused below a slot that holds one. Answers with the list. */
export async function statesSetMax(repoPath: string, max: number): Promise<StatesList> {
  return invoke("states_set_max", { repoPath, max });
}

export async function changesSubmoduleStatuses(repoPath: string): Promise<SubmoduleStatus[]> {
  return invoke("changes_submodule_statuses", { repoPath });
}

export async function changesSwitchBranch(repoPath: string, name: string): Promise<OpResult> {
  return invoke("changes_switch_branch", { repoPath, name });
}

/** `from` null = HEAD. A remote-tracking start point sets the upstream. */
export async function changesCreateBranch(
  repoPath: string,
  name: string,
  from: string | null,
  switchTo: boolean
): Promise<OpResult> {
  return invoke("changes_create_branch", { repoPath, name, from, switchTo });
}

/** `force` deletes a branch whose commits no other branch holds (the count is in the refusal). */
export async function changesDeleteBranch(repoPath: string, name: string, force: boolean): Promise<OpResult> {
  return invoke("changes_delete_branch", { repoPath, name, force });
}

export async function changesRenameBranch(repoPath: string, oldName: string, newName: string): Promise<OpResult> {
  return invoke("changes_rename_branch", { repoPath, oldName, newName });
}

/** Soft-reset HEAD~1: the last commit's changes come back staged. Refused when pushed. */
export async function changesUndoCommit(repoPath: string): Promise<OpResult> {
  return invoke("changes_undo_commit", { repoPath });
}

/** `target` is any commit id or ref, "@{u}" for the upstream. A backup branch is made when commits would leave. */
export async function changesReset(
  repoPath: string,
  target: string,
  mode: ResetMode,
  stashFirst: boolean
): Promise<OpResult> {
  return invoke("changes_reset", { repoPath, target, mode, stashFirst });
}

/** Check out a commit without moving any branch (detached HEAD). */
export async function changesDetach(repoPath: string, target: string): Promise<OpResult> {
  return invoke("changes_detach", { repoPath, target });
}

/** `mainline` (1 or 2) is only needed for a merge commit; the refusal says so and lists the parents. */
export async function changesRevert(repoPath: string, target: string, mainline: number | null): Promise<OpResult> {
  return invoke("changes_revert", { repoPath, target, mainline });
}

export async function changesCherryPick(repoPath: string, target: string): Promise<OpResult> {
  return invoke("changes_cherry_pick", { repoPath, target });
}

/** Resolve conflicted files by taking one side. "mine"/"theirs" are the user's words; the backend maps them per operation. */
export async function changesResolveSide(repoPath: string, paths: string[], side: ConflictSide): Promise<OpResult> {
  return invoke("changes_resolve_side", { repoPath, paths, side });
}

/** Everything back to HEAD; optionally deletes untracked files (never ignored ones) and stashes first. */
export async function changesDiscardAll(
  repoPath: string,
  includeUntracked: boolean,
  stashFirst: boolean
): Promise<OpResult> {
  return invoke("changes_discard_all", { repoPath, includeUntracked, stashFirst });
}

export async function changesStashList(repoPath: string): Promise<StashEntry[]> {
  return invoke("changes_stash_list", { repoPath });
}

export async function changesStashShow(repoPath: string, index: number): Promise<StashDetail> {
  return invoke("changes_stash_show", { repoPath, index });
}

export async function changesStashFileDiff(repoPath: string, index: number, path: string): Promise<FileDiff> {
  return invoke("changes_stash_file_diff", { repoPath, index, path });
}

export async function changesStashPush(
  repoPath: string,
  message: string | null,
  includeUntracked: boolean
): Promise<OpResult> {
  return invoke("changes_stash_push", { repoPath, message, includeUntracked });
}

/**
 * `oid` is the commit of the row the user saw. The list can go stale (a
 * terminal `git stash pop` shifts every index), so the backend refuses with
 * `no-stash` when stash@{index} is no longer that commit; null skips the check.
 */
export async function changesStashApply(
  repoPath: string,
  index: number,
  pop: boolean,
  restoreIndex: boolean,
  oid: string | null
): Promise<OpResult> {
  return invoke("changes_stash_apply", { repoPath, index, pop, restoreIndex, oid });
}

/** The result carries the undo command (`git stash store …`). `oid` as for apply. */
export async function changesStashDrop(repoPath: string, index: number, oid: string | null): Promise<OpResult> {
  return invoke("changes_stash_drop", { repoPath, index, oid });
}

/** `oid` as for apply. */
export async function changesStashRestoreFile(
  repoPath: string,
  index: number,
  path: string,
  oid: string | null
): Promise<OpResult> {
  return invoke("changes_stash_restore_file", { repoPath, index, path, oid });
}

/** The line diff of one file in one commit (first-parent diff for merges). */
export async function historyCommitFileDiff(repoPath: string, hash: string, path: string): Promise<FileDiff> {
  return invoke("history_commit_file_diff", { repoPath, hash, path });
}

/** What is new on the remote branch, without downloading anything. */
export interface RemotePeek {
  branch: string | null;
  upstream: string | null;
  remote: string | null;
  remote_tip: string | null;
  known_tip: string | null;
  changed: boolean;
  branch_gone: boolean;
  new_commits: number | null;
  /** "local" | "github" | null */
  counted_by: string | null;
  message: string;
}

export async function historyPeek(repoPath: string): Promise<RemotePeek> {
  return invoke("history_peek", { repoPath });
}

/** A commit id, short id or ref typed by the user; null when nothing matches. */
export async function historyResolve(repoPath: string, text: string): Promise<CommitTarget | null> {
  return invoke("history_resolve", { repoPath, text });
}

// --- Sync: the latest from upstream with your commits on top -----------------

export interface SyncCommitRef {
  oid: string;
  short: string;
  subject: string;
  /** Submodule paths whose pointer this commit moves. */
  touches: string[];
  author_name: string;
  author_email: string;
  /** The author date, ISO 8601 — stable across the rebases an update performs. */
  date: string;
  /** Changes nothing but submodule pointers (a CI bump). */
  only_pointers: boolean;
  /** Upstream already carries this change: the rebase drops it. */
  already_upstream: boolean;
  /** In an outcome: left out because nothing of it remained once its conflict was resolved. */
  resolved_away?: boolean;
}

export interface SyncSubPlan {
  path: string;
  name: string | null;
  /** none | fast-forward | rebase | ahead | blocked */
  action: string;
  reason: string;
  head: string | null;
  recorded: string;
  target: string | null;
  rebase_onto: string | null;
  predicted_gitlink_conflict: boolean;
  branch: string | null;
  detached: boolean;
  reattach_to: string | null;
  dirty_tracked: number;
  untracked: number;
  fetch_error: string | null;
  notes: string[];
  /**
   * When blocked, a stable code for the remedy (also: untracked-inside |
   * commit-first): reinit | finish-operation |
   * resolve-conflicts | removed-upstream | fetch-failed | unreachable |
   * record-pointers | squash-own | stash-inside. Buttons key on this, never
   * on the sentence.
   */
  remedy: string | null;
  /** What the update brings into this submodule, and what is its own (capped). */
  incoming_commits: SyncCommitRef[];
  own_commits: SyncCommitRef[];
  /** It has submodules of its own; the update does not go inside them. */
  has_nested: boolean;
  /** Upstream moved it to another address; the update re-points it there (git submodule sync). */
  url_change: { from: string; to: string } | null;
  /** The remote its branch's upstream lives on. */
  remote: string | null;
}

export interface SyncSuperPlan {
  branch: string;
  upstream: string;
  /** The remote the update fetches from. */
  remote?: string;
  head: string;
  upstream_oid: string;
  ahead: number;
  behind: number;
  own_commits: SyncCommitRef[];
  incoming: SyncCommitRef[];
  /** Files both sides changed: where conflicts are likely. */
  file_overlap: string[];
  dirty_count: number;
  untracked: number;
  will_stash: boolean;
  notes: string[];
  /** turn-on-stash | stash-collides | untracked-in-the-way | identity, in the blockers' order. */
  remedies: string[];
  /** Where you and upstream last agreed, shortened. */
  base_short: string;
  /** The first 30 files with uncommitted changes. */
  dirty_paths: string[];
  /** Commits beyond the 30 listed on each side. */
  more_incoming: number;
  more_own: number;
  /** Of the commits beyond the listed ones, how many upstream already has (they are dropped too). */
  more_own_dropped?: number;
}

export interface SyncSkipped {
  path: string;
  /** unmapped | not-initialised */
  why: string;
}

export interface SyncPlan {
  can_run: boolean;
  nothing_to_do: boolean;
  refusal: Refusal | null;
  blockers: string[];
  superproject: SyncSuperPlan;
  submodules: SyncSubPlan[];
  skipped: SyncSkipped[];
  /** Oids the run re-checks: ["", upstream] then [path, target] per submodule. */
  /** The superproject HEAD the plan was made for; a plan for another HEAD is stale. */
  head_oid?: string | null;
  fingerprint: [string, string][];
  summary: string;
}

export interface SyncSubOutcome {
  path: string;
  action: string;
  head_before: string | null;
  head_after: string | null;
  recorded_after: string | null;
  ok: boolean;
  note: string;
}

export interface SyncBackupRef {
  repo: string;
  branch: string;
  oid: string;
  /** The exact command that puts that repository back. */
  recovery: string;
}

export interface SyncVerified {
  behind_zero: boolean;
  own_on_top: boolean;
  dirty_paths_unchanged: boolean;
  untracked_unchanged: boolean;
  submodules_aligned: boolean;
  no_conflicts_left: boolean;
}

export interface SyncOutcome {
  /** done | needs-user | aborted | failed */
  phase: string;
  needs_user: SyncNeedsUser | null;
  backups: SyncBackupRef[];
  bundles: string[];
  head_before: string;
  head_after: string;
  incoming: number;
  own_kept: SyncCommitRef[];
  /** How many commits of yours are on top — all of them; `own_kept` lists the newest. */
  own_kept_total?: number;
  own_dropped: SyncCommitRef[];
  stashed: boolean;
  /** An autostash entry git could not re-apply is still in `git stash list`. */
  stash_left: boolean;
  submodules: SyncSubOutcome[];
  verified: SyncVerified;
  /** Submodules checked out ahead of what the branch records after the sync. */
  unrecorded_pointers: string[];
}

/** Says exactly what a sync would do; nothing is changed. `fetch` false re-plans locally after a stash toggle. */
export async function changesSyncPlan(repoPath: string, stash: boolean, fetch: boolean): Promise<SyncPlan> {
  return invoke("changes_sync_plan", { repoPath, stash, fetch });
}

/** Runs the plan; refuses when upstream moved since it was assessed (the fingerprint). */
export async function changesSyncRun(
  repoPath: string,
  stash: boolean,
  bundles: boolean,
  fingerprint: [string, string][],
  /** Download every large file still a pointer stub afterwards, in each repository that uses LFS. */
  withLfs = false
): Promise<OpResult> {
  return invoke("changes_sync_run", { repoPath, stash, bundles, fingerprint, withLfs });
}

export async function changesSyncContinue(repoPath: string, allowMarkers = false): Promise<OpResult> {
  return invoke("changes_sync_continue", { repoPath, allowMarkers });
}

export async function changesSyncAbort(repoPath: string): Promise<OpResult> {
  return invoke("changes_sync_abort", { repoPath });
}

/**
 * Opens a file of the repository in the system's default application, or
 * reveals it in the file manager. The backend checks the path is inside the
 * repository first.
 */
export async function changesOpenPath(repoPath: string, path: string, reveal: boolean): Promise<void> {
  return invoke("changes_open_path", { repoPath, path, reveal });
}

export interface ConflictHunk {
  /** 1-based line of the `<<<<<<<` marker. */
  line: number;
  mine: string[];
  /** The common ancestor's lines, when the file carries diff3 markers. */
  base: string[];
  theirs: string[];
}

export interface ConflictView {
  path: string;
  /** No hunks to show: Keep mine or Take theirs decides it. */
  binary: boolean;
  marker_count: number;
  hunks: ConflictHunk[];
  truncated: boolean;
  sides: Sides;
  total_lines: number;
  /** The file is larger than what was read: a count of zero does not mean resolved. */
  partial?: boolean;
}

/** A conflicted file split into what each side wants, for the resolution view. */
export async function changesConflictView(repoPath: string, path: string): Promise<ConflictView> {
  return invoke("changes_conflict_view", { repoPath, path });
}

/**
 * One commit recording submodule pointers checked out ahead of what the branch
 * records. `paths`: exactly these submodules — each is refused when it is not
 * a populated, listed submodule or not strictly ahead; null = every such one.
 */
export async function changesRecordPointers(repoPath: string, paths: string[] | null = null): Promise<OpResult> {
  return invoke("changes_record_pointers", { repoPath, paths });
}

export type PullMode = "ff-only" | "merge" | "rebase";

export async function changesRepoStatus(repoPath: string): Promise<RepoStatus> {
  return invoke("changes_repo_status", { repoPath });
}

export async function changesFileDiff(
  repoPath: string,
  path: string,
  staged: boolean,
  untracked: boolean
): Promise<FileDiff> {
  return invoke("changes_file_diff", { repoPath, path, staged, untracked });
}

/** `allowMarkers`: stage a conflicted file even though conflict markers are still in it (the "anyway" button). */
export async function changesStage(repoPath: string, paths: string[], allowMarkers = false): Promise<OpResult> {
  return invoke("changes_stage", { repoPath, paths, allowMarkers });
}

export async function changesStageAll(repoPath: string): Promise<OpResult> {
  return invoke("changes_stage_all", { repoPath });
}

export async function changesUnstage(repoPath: string, paths: string[]): Promise<OpResult> {
  return invoke("changes_unstage", { repoPath, paths });
}

export async function changesDiscard(repoPath: string, paths: string[]): Promise<OpResult> {
  return invoke("changes_discard", { repoPath, paths });
}

export async function changesCommit(
  repoPath: string,
  message: string,
  amend: boolean
): Promise<OpResult> {
  return invoke("changes_commit", { repoPath, message, amend });
}

/** What Publish's address check found: saved, or why not. */
export interface RemoteCheck {
  saved: boolean;
  problem: string;
  /** GitHub's new-repository form, filled in: the repository doesn't exist yet under the folder's account. */
  create_url: string | null;
  /** A non-empty repository's branches, when this branch isn't one of them: "Publish there anyway" is offered. */
  existing_branches: string[];
}

/** Connect a repository that has no remote yet to `url`, as origin, once the address is checked (Publish's "Add a remote…"). */
export async function changesAddRemote(repoPath: string, url: string, allowExisting = false): Promise<RemoteCheck> {
  return invoke("changes_add_remote", { repoPath, url, allowExisting });
}

/** A new address for `remote`, which still has the address `from` and was never reached (Publish's "Change the address…"); checked before it is saved. */
export async function changesChangeRemote(repoPath: string, remote: string, from: string, url: string, allowExisting = false): Promise<RemoteCheck> {
  return invoke("changes_change_remote", { repoPath, remote, from, url, allowExisting });
}

/** `allowUnpushedSubmodules`: push although a submodule's recorded commit is on none of that submodule's remotes. */
export async function changesPush(repoPath: string, setUpstream: boolean, allowUnpushedSubmodules = false): Promise<OpResult> {
  return invoke("changes_push", { repoPath, setUpstream, allowUnpushedSubmodules });
}

/** `withLfs` also downloads LFS content — a plain pull leaves pointer stubs. `autostash` lets a rebase pull run on a dirty tree. */
export async function changesPull(
  repoPath: string,
  mode: PullMode,
  withLfs: boolean,
  autostash: boolean,
  /** A remote-tracking branch to integrate instead of the upstream ("origin/main"). */
  source: string | null = null
): Promise<OpResult> {
  return invoke("changes_pull", { repoPath, mode, withLfs, autostash, source });
}

/** `paths`: only these submodules (the never-initialised ones); null = every listed one. */
export async function changesSubmoduleUpdate(repoPath: string, paths: string[] | null = null): Promise<OpResult> {
  return invoke("changes_submodule_update", { repoPath, paths });
}

/** Checks out, in these submodules, the commit the repository records for each. Local only. */
export async function changesSubmoduleAlign(repoPath: string, paths: string[]): Promise<OpResult> {
  return invoke("changes_submodule_align", { repoPath, paths });
}

export async function changesContinue(repoPath: string, allowMarkers = false): Promise<OpResult> {
  return invoke("changes_continue", { repoPath, allowMarkers });
}

export async function changesAbort(repoPath: string): Promise<OpResult> {
  return invoke("changes_abort", { repoPath });
}

export async function changesPushState(repoPath: string): Promise<PushState> {
  return invoke("changes_push_state", { repoPath });
}

/** "allowed" | "guardrail" | "locked". Locking and unlocking show the OS administrator prompt; the outcome may be "cancelled". */
export async function changesSetPushMode(repoPath: string, mode: PushMode): Promise<PushModeResult> {
  return invoke("changes_set_push_mode", { repoPath, mode });
}

export async function changesRepairPushLock(repoPath: string): Promise<PushModeResult> {
  return invoke("changes_repair_push_lock", { repoPath });
}

export async function pushLockHelperStatus(): Promise<HelperStatus> {
  return invoke("push_lock_helper_status");
}

export async function pushLockUninstall(): Promise<JobOutcome> {
  return invoke("push_lock_uninstall");
}

export async function pushLockFix(fix: string): Promise<JobOutcome> {
  return invoke("push_lock_fix", { fix });
}

export async function pushLockFinishManual(nonce: string): Promise<JobOutcome> {
  return invoke("push_lock_finish_manual", { nonce });
}

export async function doctorCheckPushLocks(): Promise<Finding[]> {
  return invoke("doctor_check_push_locks");
}

export async function changesPushCoverage(repoPath: string): Promise<PushCoverage> {
  return invoke("changes_push_coverage", { repoPath });
}

export async function changesRepairPushBlock(repoPath: string): Promise<PushState> {
  return invoke("changes_repair_push_block", { repoPath });
}

// --- The command log -----------------------------------------------------

/** One git command this app ran, already redacted. */
export interface ConsoleEntry {
  id: number;
  at: string;
  repo: string;
  /** Exactly what ran. `argv[0]` is the program. */
  argv: string[];
  /** -1 when git could not be started, -9 when it was killed on a timeout. */
  exit_code: number;
  duration_ms: number;
  stdout: string;
  stderr: string;
  /** A read rather than a change; hidden by default because most are. */
  is_read: boolean;
  truncated: boolean;
}

/** Commands newer than `sinceId`. An empty `repoPath` means every repository. */
export async function consoleEntries(sinceId: number, repoPath: string): Promise<ConsoleEntry[]> {
  return invoke("console_entries", { sinceId, repoPath });
}

export async function consoleClear(): Promise<void> {
  return invoke("console_clear", {});
}

// --- Terminal ------------------------------------------------------------
// The only streaming surface in the app: a shell's output arrives on a channel
// rather than as a return value, because a terminal that answered once would
// not be a terminal.

/** Start a shell in this repository. `onOutput` receives everything it prints. */
export async function terminalOpen(
  repoPath: string,
  cols: number,
  rows: number,
  onOutput: (chunk: string) => void
): Promise<string> {
  const channel = new Channel<string>();
  channel.onmessage = onOutput;
  return invoke("terminal_open", { repoPath, cols, rows, onOutput: channel });
}

export async function terminalWrite(id: string, data: string): Promise<void> {
  return invoke("terminal_write", { id, data });
}

export async function terminalResize(id: string, cols: number, rows: number): Promise<void> {
  return invoke("terminal_resize", { id, cols, rows });
}

export async function terminalClose(id: string): Promise<void> {
  return invoke("terminal_close", { id });
}

// --- GitHub: pull requests, reviews, merging, collaborators ---
// Each call names the account to act as (a RepoAccount key); the token stays in Rust.

export interface AccountAccess {
  account: string;
  login: string;
  source: string;
  permission: "admin" | "push" | "read" | "none" | "unknown";
  private: boolean | null;
  why: string | null;
}

export interface RepoContext {
  owner: string | null;
  name: string | null;
  not_github: string | null;
  accounts: AccountAccess[];
  suggested: string | null;
  profile_login: string | null;
}

export type PullState = "open" | "merged" | "closed" | "all";
export type PullScope = "created" | "review_requested" | "assigned" | "involves";

export interface PullItem {
  repo: string;
  number: number;
  title: string;
  author: string;
  draft: boolean;
  state: "open" | "merged" | "closed";
  created_at: string;
  updated_at: string;
  merged_at: string | null;
  closed_at: string | null;
  head_ref: string | null;
  base_ref: string | null;
  comments: number | null;
  labels: string[];
  html_url: string;
}

export interface PullPage {
  items: PullItem[];
  next_page: number | null;
  total: number | null;
}

export interface PullReview {
  login: string;
  state: string;
  submitted_at: string | null;
}

export interface PullCheck {
  name: string;
  state: "success" | "failure" | "pending" | "neutral" | "skipped";
  url: string | null;
}

export interface PullDetail {
  repo: string;
  number: number;
  title: string;
  body: string;
  author: string;
  state: "open" | "merged" | "closed";
  draft: boolean;
  merged_by: string | null;
  merged_at: string | null;
  mergeable: boolean | null;
  mergeable_state: string;
  head_ref: string;
  head_sha: string;
  head_repo: string | null;
  base_ref: string;
  default_branch: string;
  additions: number;
  deletions: number;
  changed_files: number;
  commits: number;
  comments: number;
  created_at: string;
  updated_at: string;
  html_url: string;
  merge_methods: ("merge" | "squash" | "rebase")[];
  merge_methods_known: boolean;
  can_push: boolean;
  is_admin: boolean;
  viewer: string;
  reviews: PullReview[];
  checks: PullCheck[];
  notes: string[];
  can_delete_branch: boolean;
}

export interface PullFile {
  path: string;
  previous_path: string | null;
  status: string;
  additions: number;
  deletions: number;
  diff: FileDiff;
}

export interface PullFiles {
  files: PullFile[];
  truncated: boolean;
}

export interface PullCommit {
  sha: string;
  title: string;
  author: string;
  date: string | null;
}

export interface MergeOutcome {
  merged: boolean;
  sha: string;
  message: string;
  branch_deleted: boolean | null;
  branch_note: string | null;
}

export interface Collaborator {
  login: string;
  role: string;
  is_owner: boolean;
}

export interface Invitation {
  id: number;
  login: string;
  role: string;
  created_at: string;
  inviter: string | null;
}

export interface People {
  collaborators: Collaborator[];
  invitations: Invitation[] | null;
  note: string | null;
}

export interface AddOutcome {
  status: "invited" | "added";
  message: string;
  invitation_id: number | null;
}

export async function githubRepoContext(repoPath: string | null, repo: string | null = null): Promise<RepoContext> {
  return invoke("github_repo_context", { repoPath, repo });
}

export async function githubPulls(account: string, owner: string, name: string, state: PullState, page = 1): Promise<PullPage> {
  return invoke("github_pulls", { account, owner, name, state, page });
}

export async function githubPullsSearch(account: string, scope: PullScope, state: PullState, repo: string | null, page = 1): Promise<PullPage> {
  return invoke("github_pulls_search", { account, scope, state, repo, page });
}

export async function githubPull(account: string, owner: string, name: string, number: number): Promise<PullDetail> {
  return invoke("github_pull", { account, owner, name, number });
}

export async function githubPullFiles(account: string, owner: string, name: string, number: number): Promise<PullFiles> {
  return invoke("github_pull_files", { account, owner, name, number });
}

export async function githubPullCommits(account: string, owner: string, name: string, number: number): Promise<PullCommit[]> {
  return invoke("github_pull_commits", { account, owner, name, number });
}

export async function githubPullReview(account: string, owner: string, name: string, number: number, event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body: string): Promise<PullReview> {
  return invoke("github_pull_review", { account, owner, name, number, event, body });
}

export async function githubPullMerge(
  account: string,
  owner: string,
  name: string,
  number: number,
  method: "merge" | "squash" | "rebase",
  sha: string,
  title: string | null,
  message: string | null,
  deleteBranch: boolean
): Promise<MergeOutcome> {
  return invoke("github_pull_merge", { account, owner, name, number, method, sha, title, message, deleteBranch });
}

export async function githubCollaborators(account: string, owner: string, name: string): Promise<People> {
  return invoke("github_collaborators", { account, owner, name });
}

export async function githubCollaboratorAdd(account: string, owner: string, name: string, username: string, permission: "pull" | "triage" | "push" | "maintain" | "admin"): Promise<AddOutcome> {
  return invoke("github_collaborator_add", { account, owner, name, username, permission });
}

export async function githubCollaboratorRemove(account: string, owner: string, name: string, username: string): Promise<void> {
  return invoke("github_collaborator_remove", { account, owner, name, username });
}

export async function githubInvitationCancel(account: string, owner: string, name: string, id: number): Promise<void> {
  return invoke("github_invitation_cancel", { account, owner, name, id });
}

/** One of the account's repositories on the Pull requests page, with its counts. */
export interface PullRepo {
  /** "owner/name" */
  repo: string;
  private: boolean;
  archived: boolean;
  fork: boolean;
  pushed_at: string | null;
  /** admin | maintain | write | triage | read */
  permission: string | null;
  /** Open pull requests; null when they could not be counted. */
  open: number | null;
  /** Open pull requests asking this account for a review; null when the count failed. */
  to_review: number | null;
  /** Open pull requests this account opened; null when the count failed. */
  mine: number | null;
  /** False: someone else's repository, known through a pull request of yours or a review asked of you. */
  affiliated: boolean;
}

export interface PullRepos {
  login: string;
  repos: PullRepo[];
  truncated: boolean;
  notes: string[];
}

export async function githubPullRepos(account: string): Promise<PullRepos> {
  return invoke("github_pull_repos", { account });
}

// ---- Uninstalling GitSwitch ----

export type AppRemoval =
  | { kind: "trash"; bundle: string; trash: string }
  | { kind: "nsis"; uninstaller: string }
  | { kind: "deb"; package: string }
  | { kind: "rpm"; package: string }
  | { kind: "app-image"; file: string }
  | { kind: "manual"; reason: string };

export interface UninstallPlan {
  platform: "macos" | "windows" | "linux";
  app: AppRemoval;
  how: string;
  gitconfig: string | null;
  signers: string | null;
  repos: string[];
  lock_helper: boolean;
  locks: string[];
  signins: string[];
  /** GitSwitch starts when the person signs in (the Net speed tab's choice). */
  startup: boolean;
  data: string[];
}

export interface UninstallPrepare {
  outcome: "done" | "stopped";
  message: string;
  command: string | null;
  done: string[];
  problems: string[];
}

export interface UninstallFinish {
  how: string;
  manual: string | null;
  log: string;
}

export async function uninstallPlan(): Promise<UninstallPlan> {
  return invoke("uninstall_plan");
}

export async function uninstallPrepare(keepData: boolean): Promise<UninstallPrepare> {
  return invoke("uninstall_prepare", { keepData });
}

/** Starts the removal and quits GitSwitch about 1.5 s later. */
export async function uninstallFinish(keepData: boolean): Promise<UninstallFinish> {
  return invoke("uninstall_finish", { keepData });
}

// ---- The Uninstall tab: apps on this computer, and their leftovers ----

export interface InstalledApp {
  id: string;
  name: string;
  version: string | null;
  publisher: string | null;
  location: string | null;
  /** app | app-store | homebrew | msi | exe | store | deb | snap | flatpak | appimage | package */
  source: string;
  size: number | null;
  date: number | null;
  notes: string[];
  blocked: string | null;
  is_self: boolean;
  /** A system app, and why (came with Windows or the PC, runs its hardware, others use it). */
  system?: string | null;
}

export interface AppList {
  platform: "macos" | "windows" | "linux";
  apps: InstalledApp[];
  notes: string[];
}

export interface AppPrepared {
  session: string;
  app: InstalledApp;
  method: string;
  command: string | null;
  facts: string[];
  also_removed: string[];
  blocked: string | null;
  running: boolean;
  warnings: string[];
  vendor_uninstaller: string | null;
  /** Windows: deleting its files directly instead of running its uninstaller. */
  force?: ForcePlan | null;
  /** Windows: why its files can't be deleted directly. */
  force_unavailable?: string | null;
}

export interface ForcePlan {
  folders: { path: string; size: number | null; size_complete: boolean; admin: boolean }[];
  /** Its entry in Installed apps. */
  entry: string;
  entry_admin: boolean;
  /** Its start-up entries, as "name (where)". */
  startup: string[];
  /** Windows asks for permission for some of it. */
  admin: boolean;
}

export interface AppProgress {
  /** uninstalling | gone | still-installed | failed */
  state: string;
  message: string | null;
}

export interface Leftover {
  id: string;
  path: string;
  place: string;
  /** file | folder | registry | launch-agent | package-settings | link */
  kind: string;
  size: number | null;
  size_complete: boolean;
  confidence: "high" | "medium";
  reason: string;
  admin: boolean;
  command: string | null;
  /** GitSwitch deletes it after Windows' permission prompt (Windows). */
  elevate?: boolean;
}

export interface LeftoverScan {
  scan: string;
  items: Leftover[];
  notes: string[];
}

export interface LeftoverDeleted {
  id: string;
  /** trashed | removed | gone | changed | refused | needs-admin | protected | in-use | at-restart | failed */
  outcome: string;
  message: string;
}

// ---- Net speed

export type NetKind = "wifi" | "ethernet" | "cellular" | "other" | "virtual";

/** An adapter, with its speed now (bytes per second). */
export interface NetIface {
  id: string;
  name: string;
  detail: string | null;
  kind: NetKind;
  up: boolean;
  in_use: boolean;
  /** Bits per second. */
  link_bps: number | null;
  down: number;
  upload: number;
}

export interface NetWifi {
  ssid: string | null;
  signal: number | null;
  rssi_dbm: number | null;
  rx_mbps: number | null;
  tx_mbps: number | null;
  /** Why something isn't shown. */
  note: string | null;
  /** The setting that would show it: "location". */
  fix: string | null;
}

export interface NetSettings {
  /** Show the speed in the taskbar / menu bar / top bar. */
  tray: boolean;
  /** Windows: numbers in the tray icon, the speed bar, or both. */
  style: "icon" | "bar" | "both";
  /** macOS / Linux: also the floating speed bar. */
  bar: boolean;
  /** Windows: the speed bar docked on the taskbar, or floating anywhere on the screen. */
  bar_place: "taskbar" | "anywhere";
  bar_pos: [number, number] | null;
  /** Windows: where on the taskbar the speed bar was dragged (kept by the bar itself). */
  bar_dock?: { anchor: "left" | "tray"; px: number } | null;
  show: "both" | "down" | "up";
  unit: "bytes" | "bits";
  interval_ms: number;
  /** "auto", "all" or an adapter's id. */
  source: string;
  /** Start GitSwitch at sign-in, without a window, to show the speed (only while it is shown). */
  start_at_login: boolean;
}

/** Starting at sign-in: the choice, and what the system has. */
export interface NetStartup {
  on: boolean;
  /** The system's entry is there and starts this GitSwitch. */
  active: boolean;
  /** The system has it turned off (Windows' Startup apps). */
  blocked: string | null;
  /** Writing or removing the entry failed. */
  error: string | null;
}

export interface NetSnapshot {
  platform: "windows" | "macos" | "linux";
  down: number;
  up: number;
  source_name: string;
  connection: NetIface | null;
  wifi: NetWifi | null;
  /** [ms since 1970, down, up], oldest first. */
  history: [number, number, number][];
  interfaces: NetIface[];
  usage: { today: [number, number]; month: [number, number]; session: [number, number]; since: number };
  settings: NetSettings;
  startup: NetStartup;
}

export interface NetBar {
  down: string;
  up: string;
  show: string;
  light: boolean;
}

export interface SpeedTest {
  /** idle | latency | download | upload | done | cancelled | failed */
  phase: string;
  progress: number;
  latency_ms: number | null;
  /** Bytes per second. */
  down_bps: number | null;
  up_bps: number | null;
  used_bytes: number;
  error: string | null;
  at: number | null;
}

export async function netspeedSnapshot(): Promise<NetSnapshot> {
  return invoke("netspeed_snapshot");
}

export async function netspeedSetSettings(settings: NetSettings): Promise<NetSettings> {
  return invoke("netspeed_set_settings", { settings });
}

export async function netspeedBar(): Promise<NetBar> {
  return invoke("netspeed_bar");
}

/** Windows: the taskbar's speed bar back beside the clock. */
export async function netspeedBarReset(): Promise<void> {
  return invoke("netspeed_bar_reset");
}

/** The system's settings page: the taskbar's icons (Windows), location access, or the apps that start at sign-in. */
export async function netspeedOpenSettings(which: "taskbar" | "location" | "startupapps" | "loginitems"): Promise<void> {
  return invoke("netspeed_open_settings", { which });
}

export async function netspeedShowTab(): Promise<void> {
  return invoke("netspeed_show_tab");
}

export async function speedtestStart(): Promise<void> {
  return invoke("netspeed_speedtest_start");
}

export async function speedtestStatus(): Promise<SpeedTest> {
  return invoke("netspeed_speedtest_status");
}

export async function speedtestCancel(): Promise<void> {
  return invoke("netspeed_speedtest_cancel");
}

/** An app uninstalled with GitSwitch before, no longer installed. */
export interface PastUninstall {
  /** The app's id: what appsRescan takes. */
  key: string;
  name: string;
  /** When it was uninstalled, seconds since 1970. */
  at: number;
}

export async function appsHistory(): Promise<PastUninstall[]> {
  return invoke("apps_history");
}

/** Look for an app's leftovers again: the session to continue with. */
export async function appsRescan(key: string): Promise<string> {
  return invoke("apps_rescan", { key });
}

/** The app's own running processes, closed — and those that would not close. */
export interface AppClosed {
  closed: string[];
  left: string[];
}

export interface UninstallSessionSummary {
  session: string;
  name: string;
  state: string;
  started: number;
}

export async function appsList(kind: "apps" | "packages"): Promise<AppList> {
  return invoke("apps_list", { kind });
}

/** [bytes, counted completely] */
export async function appsSize(id: string): Promise<[number, boolean]> {
  return invoke("apps_size", { id });
}

export async function appsPrepare(id: string): Promise<AppPrepared> {
  return invoke("apps_prepare", { id });
}

export async function appsUninstall(session: string): Promise<AppProgress> {
  return invoke("apps_uninstall", { session });
}

export async function appsStatus(session: string): Promise<AppProgress> {
  return invoke("apps_status", { session });
}

export async function appsCheckGone(session: string): Promise<AppProgress> {
  return invoke("apps_check_gone", { session });
}

export async function appsScan(session: string, registry: boolean): Promise<LeftoverScan> {
  return invoke("apps_scan", { session, registry });
}

/** Only the scan's ids travel — the backend knows what each one is. */
export async function appsDelete(scan: string, ids: string[]): Promise<LeftoverDeleted[]> {
  return invoke("apps_delete", { scan, ids });
}

export async function appsSessions(): Promise<UninstallSessionSummary[]> {
  return invoke("apps_sessions");
}

export async function appsForget(session: string): Promise<void> {
  return invoke("apps_forget", { session });
}

/** Close the app's own running processes, so its uninstall can go ahead. */
export async function appsClose(session: string): Promise<AppClosed> {
  return invoke("apps_close", { session });
}

/** Windows: leftovers in use, deleted when Windows next starts (after one permission prompt). */
export async function appsDeleteAtRestart(scan: string, ids: string[]): Promise<LeftoverDeleted[]> {
  return invoke("apps_delete_at_restart", { scan, ids });
}

/** Windows: delete a program's files directly (folders to the Recycle Bin, its entry and start-up values removed). */
export async function appsForceRemove(session: string): Promise<AppProgress> {
  return invoke("apps_force_remove", { session });
}

export async function appsOpenUninstaller(session: string): Promise<void> {
  return invoke("apps_open_uninstaller", { session });
}
