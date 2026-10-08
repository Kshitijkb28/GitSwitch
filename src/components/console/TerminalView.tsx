import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import * as api from "../../lib/api";

interface Props {
  /** The repository the shell runs in. Changing it starts a new shell. */
  repoPath: string;
  /** Bumped by the drawer when it is resized, so the shell is told the shape. */
  sizeKey: number;
  /** Visible drawers focus the shell; hidden ones keep it alive untouched. */
  visible: boolean;
}

/**
 * A real shell, drawn by xterm and fed by a pty in the backend.
 *
 * The session is deliberately keyed only on the repository: the component stays
 * mounted while the drawer is hidden so that the shell, its scrollback and
 * whatever it is running all survive being closed and reopened.
 */
export function TerminalView({ repoPath, sizeKey, visible }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  const session = useRef<string | null>(null);
  /** The size the shell was last told it has; null until it has been told. */
  const told = useRef<{ cols: number; rows: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The shell must always believe in the size that is drawn. When it believes
  // in more rows, a full-screen program paints its last lines below the
  // visible edge and they cannot be reached until something resizes the
  // drawer. So every change of the terminal's size is passed on — and a size
  // that changed before the shell existed is passed on the moment it starts.
  const tell = (cols: number, rows: number) => {
    const id = session.current;
    if (!id) return; // no shell yet: the start of the session syncs
    if (told.current && told.current.cols === cols && told.current.rows === rows) return;
    told.current = { cols, rows };
    void api.terminalResize(id, cols, rows).catch(() => {
      // Not delivered: forget it, so the next fit says it again.
      if (told.current && told.current.cols === cols && told.current.rows === rows) told.current = null;
    });
  };

  // Fit the rows to the box. The terminal reports its own change (`onResize`),
  // which is what tells the shell.
  const refit = () => {
    const t = term.current;
    const f = fit.current;
    if (!t || !f) return;
    try {
      f.fit();
    } catch {
      return;
    }
    // The same size as before reports nothing; say it anyway if the shell
    // has not heard it (a resize that could not be delivered, a late start).
    tell(t.cols, t.rows);
  };

  // One shell per repository. A new repo means a new shell, and the old one is
  // closed rather than left running in a folder nobody is looking at.
  useEffect(() => {
    if (!host.current || !repoPath) return;
    let dead = false;
    // A new shell starts clean: an error from the folder before (one deleted
    // since, say) must not stay on screen over this one.
    setError(null);

    const t = new Terminal({
      fontSize: 12,
      fontFamily:
        'ui-monospace, SFMono-Regular, Menlo, Monaco, "Cascadia Mono", "Roboto Mono", monospace',
      cursorBlink: true,
      convertEol: false,
      scrollback: 5000,
      theme: {
        background: "#18181b", // zinc-900, matching every other surface
        foreground: "#e4e4e7",
        cursor: "#34d399",
        selectionBackground: "#3f3f46",
        black: "#27272a",
        red: "#f87171",
        green: "#34d399",
        yellow: "#fbbf24",
        blue: "#60a5fa",
        magenta: "#c084fc",
        cyan: "#22d3ee",
        white: "#e4e4e7",
      },
    });
    const f = new FitAddon();
    t.loadAddon(f);
    t.open(host.current);
    try {
      f.fit();
    } catch {
      // the drawer may not have been laid out yet; the resize effect retries
    }
    term.current = t;
    fit.current = f;
    told.current = null;
    // Whoever changes the terminal's size — the fit, or anything else — the
    // shell hears it from here.
    t.onResize(({ cols, rows }) => tell(cols, rows));

    // The size the shell is started with; it may be stale by the time the
    // shell exists.
    const opened = { cols: t.cols, rows: t.rows };
    api
      .terminalOpen(repoPath, opened.cols, opened.rows, (chunk) => {
        if (!dead) t.write(chunk);
      })
      .then((id) => {
        if (dead) {
          void api.terminalClose(id);
          return;
        }
        session.current = id;
        told.current = opened;
        t.onData((data) => {
          void api.terminalWrite(id, data).catch(() => {
            /* the shell exited; its farewell is already on screen */
          });
        });
        // Fit once more now that there is a shell to tell, and pass on
        // whatever the size became while it was starting.
        refit();
      })
      .catch((e) => {
        if (!dead) setError(String(e));
      });

    return () => {
      dead = true;
      const id = session.current;
      session.current = null;
      told.current = null;
      if (id) void api.terminalClose(id);
      t.dispose();
      term.current = null;
      fit.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repoPath]);

  // The window changed shape, or the drawer was dragged.
  useEffect(() => {
    if (!visible) return;
    const id = window.setTimeout(refit, 50);
    return () => window.clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sizeKey, visible]);

  // The first fit happens the moment the terminal mounts, before its font
  // and first paint have settled, and comes out with too many rows — the
  // last lines then sit below the box until something resizes it. Fit again
  // once the drawer has opened and once the fonts are in.
  useEffect(() => {
    if (!visible) return;
    const ids = [300, 1000].map((ms) => window.setTimeout(refit, ms));
    let stale = false;
    document.fonts?.ready.then(() => {
      if (!stale) refit();
    });
    return () => {
      stale = true;
      ids.forEach((id) => window.clearTimeout(id));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible]);

  // The box itself changes shape without the window doing so: the drawer's
  // header wraps onto two lines at a narrow width, the drawer is dragged,
  // a banner appears. Fit to the box, whatever changed it — or the last
  // rows sit below the visible edge and cannot be scrolled to.
  useEffect(() => {
    const el = host.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    let id = 0;
    const ro = new ResizeObserver(() => {
      window.clearTimeout(id);
      id = window.setTimeout(() => {
        if (visible) refit();
      }, 50);
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      window.clearTimeout(id);
    };
  }, [visible]);

  useEffect(() => {
    if (visible) term.current?.focus();
  }, [visible]);

  // The error has a line of its own above the shell — drawn over the shell
  // it hid the prompt — and the shell is fitted to what is left.
  return (
    <div className="flex h-full w-full min-h-0 flex-col">
      {error && (
        <p role="alert" className="shrink-0 break-words bg-red-500/10 px-3 py-2 text-xs text-red-300">
          {error}
        </p>
      )}
      <div ref={host} className="min-h-0 w-full flex-1" />
    </div>
  );
}
