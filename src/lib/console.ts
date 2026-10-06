import { useSyncExternalStore } from "react";
import { readPersisted, writePersisted } from "./persist";

/**
 * Whether the terminal drawer is open, how tall it is, and which repository it
 * is pointed at.
 *
 * This lives outside React for the same reason `gitJobs.ts` does: the drawer is
 * rendered once at the app level and must survive every page change, and the
 * shell behind it is a live process that would be killed by an unmount. A
 * module store also means any page can open the drawer without threading props
 * through the whole tree.
 */
export type ConsoleTab = "terminal" | "activity";

export type ConsoleState = {
  open: boolean;
  /** Which panel is on screen. Both stay mounted; see ConsoleDrawer. */
  tab: ConsoleTab;
  /** Profile id the repository list is filtered by; "" means every account. */
  account: string;
  /** Absolute path of the repository the shell runs in. */
  repo: string;
  /** Drawer height in pixels. */
  height: number;
};

const MIN_HEIGHT = 140;
const MAX_HEIGHT = 900;

let state: ConsoleState = {
  open: false, // never restored open: a shell should start when asked for
  tab: readPersisted<ConsoleTab>("console.tab", "terminal"),
  account: readPersisted<string>("console.account", "") || readPersisted<string>("changes.account", ""),
  repo: readPersisted<string>("console.repo", "") || readPersisted<string>("changes.repo", ""),
  height: clampHeight(readPersisted<number>("console.height", 300)),
};

const listeners = new Set<() => void>();

export function clampHeight(px: number): number {
  if (!Number.isFinite(px)) return 300;
  return Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round(px)));
}

function emit() {
  for (const l of listeners) l();
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function getConsole(): ConsoleState {
  return state;
}

export function useConsole(): ConsoleState {
  return useSyncExternalStore(subscribe, getConsole, getConsole);
}

function set(next: Partial<ConsoleState>) {
  state = { ...state, ...next };
  emit();
}

/** Open the drawer, optionally pointing it at a repository. */
export function openConsole(repo?: string) {
  if (repo && repo !== state.repo) writePersisted("console.repo", repo);
  set({ open: true, ...(repo ? { repo } : {}) });
}

export function closeConsole() {
  set({ open: false });
}

export function toggleConsole() {
  if (state.open) closeConsole();
  else openConsole();
}

export function setConsoleRepo(repo: string) {
  writePersisted("console.repo", repo);
  set({ repo });
}

export function setConsoleTab(tab: ConsoleTab) {
  writePersisted("console.tab", tab);
  set({ tab });
}

export function setConsoleAccount(account: string) {
  writePersisted("console.account", account);
  set({ account });
}

export function setConsoleHeight(px: number) {
  const height = clampHeight(px);
  writePersisted("console.height", height);
  set({ height });
}
