import { useSyncExternalStore } from "react";
import { readPersisted, writePersisted } from "./persist";
import { readSelectedRepo } from "./repoSelection";

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
  /** Bumped when push access was changed in the app; the badge re-reads it. Not persisted. */
  pushRev: number;
};

const MIN_HEIGHT = 140;
const MAX_HEIGHT = 900;
/** The page keeps at least this much above the drawer: its own header stays usable. */
const PAGE_ABOVE = 120;

let state: ConsoleState = {
  open: false, // never restored open: a shell should start when asked for
  tab: readPersisted<ConsoleTab>("console.tab", "terminal"),
  account: readPersisted<string>("console.account", "") || readPersisted<string>("changes.account", ""),
  repo: readPersisted<string>("console.repo", "") || readPersisted<string>("changes.repo", ""),
  height: clampHeight(readPersisted<number>("console.height", 300)),
  pushRev: 0,
};

const listeners = new Set<() => void>();

/** The tallest the drawer may be in this window — never past its top edge. */
export function maxHeight(windowHeight: number = typeof window === "undefined" ? Infinity : window.innerHeight): number {
  if (!Number.isFinite(windowHeight) || windowHeight <= 0) return MAX_HEIGHT;
  return Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, Math.floor(windowHeight - PAGE_ABOVE)));
}

/**
 * A height that fits this window. A remembered height (or one dragged on
 * a taller screen) past the window's top would carry the drawer's header,
 * its pickers and its close button out of reach.
 */
export function clampHeight(px: number, windowHeight?: number): number {
  const top = maxHeight(windowHeight);
  if (!Number.isFinite(px)) return Math.min(300, top);
  return Math.min(top, Math.max(MIN_HEIGHT, Math.round(px)));
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

/**
 * The page selection the drawer last took over. Empty at start, so the first
 * open of every run takes the page's folder; after that the drawer follows the
 * page only when the page has moved on, and a repository picked inside the
 * drawer survives closing and reopening it.
 */
let followed = "";

/** Point the drawer at the page's folder, if the page moved since last time. */
function followPage() {
  const selected = readSelectedRepo();
  if (!selected || selected === followed) return;
  followed = selected;
  // Every account: the page keeps a folder under any account filter, and the
  // drawer's own filter would otherwise swap it for one of that account's.
  writePersisted("console.repo", selected);
  writePersisted("console.account", "");
  set({ repo: selected, account: "" });
}

/** Open the drawer, optionally pointing it at a repository. */
export function openConsole(repo?: string) {
  if (repo) {
    followed = readSelectedRepo();
    writePersisted("console.repo", repo);
    writePersisted("console.account", "");
    set({ open: true, repo, account: "" });
    return;
  }
  set({ open: true });
}

export function closeConsole() {
  set({ open: false });
}

/** The sidebar button and Ctrl+`: open in the page's folder, or close. */
export function toggleConsole() {
  if (state.open) {
    closeConsole();
    return;
  }
  followPage();
  openConsole();
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

/**
 * Push access was changed in the app (a mode, a repair, a lock job): the
 * drawer's badge reads it again. A signal, never a poll — reading push state
 * under a lock re-applies drifted mirrors as a side effect.
 */
export function notePushChanged() {
  set({ pushRev: state.pushRev + 1 });
}

export function setConsoleHeight(px: number) {
  const height = clampHeight(px);
  writePersisted("console.height", height);
  set({ height });
}

/** The window changed shape: a drawer now taller than fits is brought back in. */
export function fitConsoleToWindow() {
  const height = clampHeight(state.height);
  if (height !== state.height) set({ height });
}
