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
  profile_name: string;
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
  search?: string | null
): Promise<HistoryPage> {
  return invoke("history_page", { repoPath, rev, offset, limit, search: search ?? null });
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

export async function sparseClone(
  url: string,
  parentDir: string,
  folderName?: string | null
): Promise<string> {
  return invoke("sparse_clone", {
    url,
    parentDir,
    folderName: folderName ?? null,
  });
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
