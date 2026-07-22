// The GitHub OAuth App Client ID used for the device-flow sign-in.
// Client IDs are public (not secrets). A working default is baked in; users can
// override it with their own OAuth App in Settings.
export const OAUTH_CLIENT_ID_KEY = "gitswitch_oauth_client_id";

// Default GitSwitch OAuth App (Device Flow enabled). One Client ID lets ANY
// GitHub account authenticate — it identifies the app, not an account.
export const DEFAULT_OAUTH_CLIENT_ID = "Ov23lieKPekuiZoTUNvb";

/** The user's custom Client ID override, or null if they haven't set one. */
export function getCustomClientId(): string | null {
  const v = localStorage.getItem(OAUTH_CLIENT_ID_KEY);
  return v && v.trim() ? v.trim() : null;
}

/** The Client ID the device flow should actually use (custom override, else default). */
export function getEffectiveClientId(): string {
  return getCustomClientId() ?? DEFAULT_OAUTH_CLIENT_ID;
}

export function setOAuthClientId(id: string): void {
  localStorage.setItem(OAUTH_CLIENT_ID_KEY, id.trim());
}
