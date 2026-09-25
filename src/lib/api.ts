import { invoke as tauriInvoke } from "@tauri-apps/api/core";
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

export async function createProfile(params: {
  name: string;
  gitName: string;
  gitEmail: string;
  sshKeyPath?: string | null;
  directories: string[];
  allowPush?: boolean | null;
  signingEnabled?: boolean | null;
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

export async function ghRegisterSshKey(
  account: string,
  keyPath: string,
  title: string
): Promise<string> {
  return invoke("gh_register_ssh_key", { account, keyPath, title });
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
  lane: number;
  parent_lanes: number[];
  active_lanes: number[];
}

export interface HistoryPage {
  commits: HistoryCommit[];
  total: number;
  offset: number;
  limit: number;
  has_more: boolean;
  max_lane: number;
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

export async function historySyncStatus(
  repoPath: string,
  branch: string
): Promise<SyncStatus> {
  return invoke("history_sync_status", { repoPath, branch });
}

export interface MergeInfo {
  merged_into: string[];
  merges: HistoryCommit[];
}

export async function historyListRepos(): Promise<RepoRef[]> {
  return invoke("history_list_repos");
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
  author?: string | null
): Promise<HistoryPage> {
  return invoke("history_page", {
    repoPath,
    rev,
    offset,
    limit,
    search: search ?? null,
    author: author ?? null,
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

export async function doctorFixSshConfig(): Promise<string> {
  return invoke("doctor_fix_ssh_config");
}

export interface ScannedRepo {
  path: string;
  remote_url: string;
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

export async function checkRepoAccess(
  token: string,
  owner: string,
  repo: string
): Promise<RepoPermissions | null> {
  return invoke("check_repo_access", { token, owner, repo });
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
}

export interface SparseSetResult {
  lfs: LfsReport | null;
}

export async function lfsAvailable(): Promise<LfsTool> {
  return invoke("lfs_available");
}

/** `cloneAs` = profile id to authenticate as; null lets the folder decide. */
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
  /** "gh:<account>" or "profile:<profile id>" */
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

export async function storeGithubToken(
  profileId: string,
  token: string
): Promise<void> {
  return invoke("store_github_token", { profileId, token });
}

export async function getGithubToken(
  profileId: string
): Promise<string | null> {
  return invoke("get_github_token", { profileId });
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
  email: string | null;
  avatar_url: string;
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
}

export interface SyncNeedsUser {
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

export type JobOutcomeKind =
  | "applied"
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
    | "unmapped";
  summary: string;
  gitdir_valid: boolean;
  operation: string | null;
  conflicted: number;
  upstream_remote: string | null;
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
  /** switch | create-branch | delete-branch | rename-branch | undo-commit | reset | detach | revert | cherry-pick | resolve-side | discard-all */
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
}

export interface SyncSuperPlan {
  branch: string;
  upstream: string;
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
  fingerprint: [string, string][]
): Promise<OpResult> {
  return invoke("changes_sync_run", { repoPath, stash, bundles, fingerprint });
}

export async function changesSyncContinue(repoPath: string): Promise<OpResult> {
  return invoke("changes_sync_continue", { repoPath });
}

export async function changesSyncAbort(repoPath: string): Promise<OpResult> {
  return invoke("changes_sync_abort", { repoPath });
}

/** One commit recording every submodule checked out ahead of what the branch records. */
export async function changesRecordPointers(repoPath: string): Promise<OpResult> {
  return invoke("changes_record_pointers", { repoPath });
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

export async function changesStage(repoPath: string, paths: string[]): Promise<OpResult> {
  return invoke("changes_stage", { repoPath, paths });
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

export async function changesPush(repoPath: string, setUpstream: boolean): Promise<OpResult> {
  return invoke("changes_push", { repoPath, setUpstream });
}

/** `withLfs` also downloads LFS content — a plain pull leaves pointer stubs. `autostash` lets a rebase pull run on a dirty tree. */
export async function changesPull(
  repoPath: string,
  mode: PullMode,
  withLfs: boolean,
  autostash: boolean
): Promise<OpResult> {
  return invoke("changes_pull", { repoPath, mode, withLfs, autostash });
}

export async function changesSubmoduleUpdate(repoPath: string): Promise<OpResult> {
  return invoke("changes_submodule_update", { repoPath });
}

export async function changesContinue(repoPath: string): Promise<OpResult> {
  return invoke("changes_continue", { repoPath });
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

export async function changesRepairPushBlock(repoPath: string): Promise<PushState> {
  return invoke("changes_repair_push_block", { repoPath });
}
