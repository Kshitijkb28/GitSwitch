import { useCallback, useMemo } from "react";
import { readPersisted, usePersistedState, writePersisted } from "./persist";
import { canonicalJoin, isWithin, normPath } from "./paths";

/**
 * The folder the Repository page is on right now, its submodule scope
 * included — read at call time, for code outside the page (the terminal
 * drawer). The hook and `handOff` write these keys as the selection changes.
 */
export function readSelectedRepo(): string {
  const path = readPersisted<string>("repo.path", "");
  if (path) return canonicalJoin(path, readPersisted<string>("repo.sub", ""));
  return readPersisted<string>("changes.repo", "") || readPersisted<string>("history.repo", "");
}

/**
 * The one repository selection the Repository page works on: an account
 * filter, a repository, and optionally a submodule inside it ("scope").
 *
 * Persisted as repo.account / repo.path / repo.sub. The older keys the two
 * separate pages used (changes.repo, changes.account, history.repo,
 * history.sub, history.account) are written through for one release, so the
 * terminal drawer (console.ts reads changes.repo), `handOff` and anything else
 * that remembers the old names keep agreeing with this page.
 */
export function useRepoSelection() {
  const [account, setAccountRaw] = usePersistedState(
    "repo.account",
    readPersisted<string>("changes.account", "") || readPersisted<string>("history.account", "")
  );
  const [path, setPathRaw] = usePersistedState(
    "repo.path",
    readPersisted<string>("changes.repo", "") || readPersisted<string>("history.repo", "")
  );
  const [sub, setSubRaw] = usePersistedState("repo.sub", readPersisted<string>("history.sub", ""));

  const effectivePath = useMemo(() => (path ? canonicalJoin(path, sub) : ""), [path, sub]);

  const mirror = (p: string, s: string, a: string) => {
    writePersisted("changes.repo", p ? canonicalJoin(p, s) : "");
    writePersisted("history.repo", p);
    writePersisted("history.sub", s);
    writePersisted("changes.account", a);
    writePersisted("history.account", a);
  };

  const setAccount = useCallback(
    (a: string) => {
      setAccountRaw(a);
      mirror(path, sub, a);
    },
    [path, sub, setAccountRaw]
  );
  /** A new repository forgets the submodule scope: it belonged to the old one. */
  const setPath = useCallback(
    (p: string) => {
      setPathRaw(p);
      setSubRaw("");
      mirror(p, "", account);
    },
    [account, setPathRaw, setSubRaw]
  );
  const setSub = useCallback(
    (s: string) => {
      setSubRaw(s);
      mirror(path, s, account);
    },
    [account, path, setSubRaw]
  );
  /**
   * Open an absolute folder: a submodule of the current repository becomes the
   * scope, anything else becomes the repository (what "Work in this submodule
   * as its own repository" and "Open a folder…" did on the Changes page).
   * With `scopes` — the repository's gitlink paths — only one of those becomes
   * the scope; any other folder inside is opened as a repository of its own.
   */
  const openFolder = useCallback(
    (abs: string, scopes?: readonly string[]) => {
      const target = normPath(abs);
      const root = normPath(path);
      if (path && target !== root && isWithin(target, path)) {
        const rel = target.slice(root.length + 1);
        if (!scopes || scopes.includes(rel)) {
          setSub(rel);
          return;
        }
      }
      setPath(target);
    },
    [path, setPath, setSub]
  );

  return { account, path, sub, effectivePath, setAccount, setPath, setSub, openFolder };
}
