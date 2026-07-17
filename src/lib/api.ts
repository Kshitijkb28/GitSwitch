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
