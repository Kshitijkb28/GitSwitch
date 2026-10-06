import { useCallback, useState } from "react";

const PREFIX = "gitswitch:";

/** Read a persisted value from outside a component — for module-scope stores
 *  like the console drawer, which exist before any page mounts. */
export function readPersisted<T>(key: string, fallback: T): T {
  return read(key, fallback);
}

function read<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback; // corrupt or unreadable — fall back rather than crash
  }
}

/**
 * useState that survives navigation and app restarts.
 *
 * Pages are unmounted when you switch tabs, so plain useState loses the repo
 * you picked, the branch you were on, the folder you browsed to… Persisting
 * the few "where was I" values makes coming back to a page feel like
 * returning rather than starting over.
 *
 * Only durable selections belong here — never transient things like loading
 * flags, fetched results or error messages, which must be re-derived fresh.
 */
/** Write a persisted value from outside its page — e.g. the Clone page
 *  handing a fresh clone to the Changes page before navigating there. */
export function writePersisted<T>(key: string, value: T) {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // storage full or disabled — the page will just not be preselected
  }
}

/**
 * Hand a repository to another page before navigating there. It sets that
 * page's repository AND clears its narrower selection (History's submodule),
 * so a sub-selection left over from an earlier visit cannot hijack the
 * hand-off and show a different repository than the one that was sent.
 */
export function handOff(page: "changes" | "history", repo: string) {
  writePersisted(`${page}.repo`, repo);
  if (page === "history") writePersisted("history.sub", "");
  // Both pages now live on the Repository page, which reads these keys.
  writePersisted("repo.path", repo);
  writePersisted("repo.sub", "");
}

export function usePersistedState<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => read(key, initial));

  const set = useCallback(
    (next: T | ((prev: T) => T)) => {
      setValue((prev) => {
        const v = typeof next === "function" ? (next as (p: T) => T)(prev) : next;
        try {
          localStorage.setItem(PREFIX + key, JSON.stringify(v));
        } catch {
          // storage full or disabled — keep working, just don't persist
        }
        return v;
      });
    },
    [key]
  );

  return [value, set] as const;
}
