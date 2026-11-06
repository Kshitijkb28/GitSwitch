import { useSyncExternalStore } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";

/**
 * The newest GitSwitch release found, shared by the sidebar (which says one
 * is waiting) and Settings (which installs it). The app checks a little after
 * it starts and then every few hours, since it lives in the tray for days;
 * Settings' button checks on demand.
 *
 * The release feed is latest.json on the latest published GitHub release
 * (tauri.conf.json); each copy picks the entry for the installer it came
 * from, so updating stays within that kind of install.
 */
export type UpdateState = {
  update: Update | null;
  checking: boolean;
  /** The last check's failure, said in words; null after a good check. */
  error: string | null;
};

const FIRST_CHECK_MS = 10_000;
const EVERY_MS = 6 * 60 * 60 * 1000;

let state: UpdateState = { update: null, checking: false, error: null };
let inFlight: Promise<Update | null> | null = null;
const listeners = new Set<() => void>();

function set(next: Partial<UpdateState>) {
  state = { ...state, ...next };
  for (const l of listeners) l();
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function useUpdates(): UpdateState {
  return useSyncExternalStore(subscribe, () => state);
}

/** The updater's errors, as something a person can act on. */
export function updateErrorText(e: unknown): string {
  const msg = String(e);
  if (/valid release JSON|404/i.test(msg)) return "No GitSwitch release has been published yet.";
  if (/platform.*not found|fallback platforms/i.test(msg)) return "The latest release has no update for this kind of install yet.";
  if (/endpoints|pubkey/i.test(msg)) return "Update checks aren't configured for this build (see BUILD.md, \"Auto-updater releases\").";
  if (/Tauri runtime|__TAURI_INTERNALS__/i.test(msg)) return "Updates are checked in the desktop app.";
  if (/error sending request|dns|network|connect|timed out/i.test(msg)) return "Could not reach GitHub to check for updates.";
  return `Could not check for updates: ${msg}`;
}

/**
 * Ask GitHub for a newer release. Two callers at once share one request.
 * Resolves to the update (also kept for the sidebar and Settings) or null;
 * rejects with the reason in words.
 */
export function checkForUpdate(): Promise<Update | null> {
  if (inFlight) return inFlight;
  set({ checking: true });
  inFlight = check()
    .then((update) => {
      // The previous handle is left open: Settings may be installing from it.
      set({ update, checking: false, error: null });
      return update;
    })
    .catch((e) => {
      const error = updateErrorText(e);
      set({ checking: false, error });
      throw new Error(error);
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

/** Check soon after launch and every few hours after; stops when called back. */
export function startUpdateChecks(): () => void {
  const quiet = () => void checkForUpdate().catch(() => {});
  const first = setTimeout(quiet, FIRST_CHECK_MS);
  const every = setInterval(quiet, EVERY_MS);
  return () => {
    clearTimeout(first);
    clearInterval(every);
  };
}
