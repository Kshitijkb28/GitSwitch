/**
 * Changes GitSwitch itself made to repositories on GitHub: one made private
 * or public, one deleted, one created (or made again under a name GitHub had
 * lost). The pages that keep a listing in memory (the Repositories page, the
 * Pull requests page) listen, so a row shows what GitHub has now without
 * asking GitHub for the whole list again; the Repository page's GitHub tab
 * listens for its repository being made again.
 */
export type RepoChange =
  | { kind: "visibility"; fullName: string; private: boolean; visibility: "public" | "private" | "internal" }
  | ({ kind: "deleted" } & Deleted)
  | { kind: "created"; fullName: string; private: boolean };

/**
 * A repository deleted on GitHub from GitSwitch: its name as GitHub had it,
 * GitHub's answer as the backend words it (who can restore it, and from
 * where), and where that is on github.com — null when it can't be restored
 * from settings (it was part of a fork network). GitSwitch itself can't
 * restore it: GitHub offers no way but its own settings page.
 */
export type Deleted = { fullName: string; message: string; restoreUrl: string | null };

type Listener = (change: RepoChange) => void;

const listeners = new Set<Listener>();

/** The changes made in this session, with when: a listing asked for before one doesn't show it yet. */
const made: { change: RepoChange; at: number }[] = [];
const KEEP = 100;

/** Listen for changes; returns the way to stop. */
export function onRepoChange(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Tell every listener; one that fails does not stop the others. */
export function notifyRepoChange(change: RepoChange) {
  made.push({ change, at: Date.now() });
  if (made.length > KEEP) made.splice(0, made.length - KEEP);
  for (const listener of [...listeners]) {
    try {
      listener(change);
    } catch {
      // a listener's own problem; the others still hear of it
    }
  }
}

/** The changes made since `since` (a Date.now() time), oldest first: what a listing asked for at `since` may still show the old way. */
export function changesSince(since: number): RepoChange[] {
  return made.filter((m) => m.at >= since).map((m) => m.change);
}

/** The same repository, letter case aside ("Owner/Name" and "owner/name"). */
export const sameRepo = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** The last change made in this session to `fullName`, or null: a page opened later reads what was done before it. */
export function lastChangeTo(fullName: string): RepoChange | null {
  for (let i = made.length - 1; i >= 0; i--) if (sameRepo(made[i].change.fullName, fullName)) return made[i].change;
  return null;
}
