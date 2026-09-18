import { useCallback, useState } from "react";

const PREFIX = "gitswitch:";

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
