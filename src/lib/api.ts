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
  error: string | null;
}

export interface CloneResult {
  path: string;
  /** Profile whose SSH key authenticated the clone. */
  profile_name: string | null;
  /** Set when the new folder was added to that profile. */
  mapped_to: string | null;
  submodules: SubmoduleReport | null;
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
  withSubmodules = true
): Promise<CloneResult> {
  return invoke("full_clone", {
    url,
    parentDir,
    folderName: folderName ?? null,
    cloneAs: cloneAs || null,
    withSubmodules,
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
  dirs: string[]
): Promise<void> {
  return invoke("sparse_set", { repoPath, dirs });
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
  staged_added: number | null;
  staged_removed: number | null;
  unstaged_added: number | null;
  unstaged_removed: number | null;
  is_binary: boolean;
}

export interface InProgress {
  kind: string;
  label: string;
  detail: string;
  abort_command: string;
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
}

export interface SubmoduleReport {
  listed: number;
  downloaded: number;
  unlisted: number;
  error: string | null;
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

export async function changesPull(repoPath: string, mode: PullMode): Promise<OpResult> {
  return invoke("changes_pull", { repoPath, mode });
}

export async function changesSubmoduleUpdate(repoPath: string): Promise<OpResult> {
  return invoke("changes_submodule_update", { repoPath });
}

export async function changesAbort(repoPath: string): Promise<OpResult> {
  return invoke("changes_abort", { repoPath });
}

export async function changesPushState(repoPath: string): Promise<PushState> {
  return invoke("changes_push_state", { repoPath });
}

export async function changesSetPushBlocked(
  repoPath: string,
  blocked: boolean
): Promise<PushState> {
  return invoke("changes_set_push_blocked", { repoPath, blocked });
}

export async function changesRepairPushBlock(repoPath: string): Promise<PushState> {
  return invoke("changes_repair_push_block", { repoPath });
}
