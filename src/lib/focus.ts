import { useEffect, useRef } from "react";

/**
 * Run `cb` whenever the user comes back to the app.
 *
 * Folders can be deleted, moved or cloned behind the app's back, so anything
 * showing local disk state has to re-check on return rather than trust what it
 * read when the page first opened. Throttled, because returning to a window can
 * fire several of these events at once.
 */
export function useRefreshOnFocus(cb: () => void, minIntervalMs = 1500) {
  const latest = useRef(cb);
  latest.current = cb;

  useEffect(() => {
    let last = 0;
    const run = () => {
      const now = Date.now();
      if (now - last < minIntervalMs) return;
      last = now;
      latest.current();
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") run();
    };

    window.addEventListener("focus", run);
    document.addEventListener("visibilitychange", onVisible);

    // A Tauri window doesn't always emit DOM focus events when the app is
    // re-activated, so listen to the native signal as well.
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    if (typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__) {
      import("@tauri-apps/api/window")
        .then(({ getCurrentWindow }) =>
          getCurrentWindow().onFocusChanged(({ payload: focused }) => {
            if (focused) run();
          })
        )
        .then((u) => {
          if (cancelled) u();
          else unlisten = u;
        })
        .catch(() => {
          /* not in a Tauri window — the DOM events above are enough */
        });
    }

    return () => {
      cancelled = true;
      window.removeEventListener("focus", run);
      document.removeEventListener("visibilitychange", onVisible);
      unlisten?.();
    };
  }, [minIntervalMs]);
}
