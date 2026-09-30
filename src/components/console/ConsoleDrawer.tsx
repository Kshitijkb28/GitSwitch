import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { X, SquareTerminal, Info, ShieldAlert, Lock } from "lucide-react";
import { Select } from "../Select";
import { Badge } from "../Badge";
import { ActivityView } from "./ActivityView";
import { currentMode } from "../changes/PushAccessCard";
// xterm is ~250 KB and only a person who opens the drawer needs it, so it is
// fetched on first open rather than shipped in the main bundle.
const TerminalView = lazy(() =>
  import("./TerminalView").then((m) => ({ default: m.TerminalView }))
);
import * as api from "../../lib/api";
import {
  closeConsole,
  setConsoleAccount,
  setConsoleTab,
  setConsoleHeight,
  setConsoleRepo,
  useConsole,
} from "../../lib/console";
import { baseName } from "../../lib/paths";

/**
 * A shell, in a drawer, pinned to the bottom of the window on every page.
 *
 * It is rendered at the app level rather than inside a page for two reasons:
 * the shell must survive moving between pages, and a `fixed` element sits
 * outside the page's own scroll box, so a terminal can never push a page
 * sideways at a narrow window.
 *
 * Once opened it is kept mounted and merely hidden, because unmounting would
 * kill a shell that might be halfway through something.
 */
export function ConsoleDrawer() {
  const { open, tab, account, repo, height } = useConsole();
  const [repos, setRepos] = useState<api.RepoRef[]>([]);
  const [started, setStarted] = useState(false);
  const [sizeKey, setSizeKey] = useState(0);
  const [showNote, setShowNote] = useState(false);
  const [push, setPush] = useState<api.PushState | null>(null);
  const dragging = useRef(false);
  const pushSeq = useRef(0);

  useEffect(() => {
    if (open) setStarted(true);
  }, [open]);

  // The repository list is read once the drawer is first opened, not on app
  // start: a drawer nobody opens should cost nothing.
  useEffect(() => {
    if (!started) return;
    let cancelled = false;
    api
      .historyListRepos()
      .then((list) => {
        if (!cancelled) setRepos(list);
      })
      .catch(() => {
        /* the picker stays empty; the drawer still shows why */
      });
    return () => {
      cancelled = true;
    };
  }, [started]);

  // The accounts are whichever profiles own the repositories on this machine —
  // the same derivation the Changes page uses, so the two agree.
  const accounts = useMemo(
    () =>
      Array.from(
        new Map(repos.map((r) => [r.profile_id, { id: r.profile_id, name: r.profile_name }])).values()
      ),
    [repos]
  );
  const visible = useMemo(
    () => (account ? repos.filter((r) => r.profile_id === account) : repos),
    [repos, account]
  );

  // Never leave the picker naming a repository the filter just hid, or the
  // shell and the label would disagree about where you are.
  useEffect(() => {
    if (!visible.length) return;
    if (!repo || !visible.some((r) => r.path === repo)) setConsoleRepo(visible[0].path);
  }, [visible, repo]);

  // What this repository's push state actually is, so it is known before a
  // push is typed rather than discovered from a refusal.
  //
  // `changesPushState` rather than the full status: it is ~10 git config and
  // remote reads that never touch the object database, so it costs the same on
  // a monorepo. It is fetched on a repo change only and never polled, because
  // under a lock it re-applies drifted mirrors as a side effect.
  useEffect(() => {
    if (!started || !open || !repo) {
      setPush(null);
      return;
    }
    // The account filter re-selects a repository when it hides the current
    // one, so `repo` can change twice in a breath; the last request wins.
    const mine = ++pushSeq.current;
    const id = window.setTimeout(() => {
      api
        .changesPushState(repo)
        .then((p) => {
          if (mine === pushSeq.current) setPush(p);
        })
        .catch(() => {
          if (mine === pushSeq.current) setPush(null);
        });
    }, 200);
    return () => window.clearTimeout(id);
  }, [started, open, repo]);

  const onDrag = useCallback((e: MouseEvent) => {
    if (!dragging.current) return;
    setConsoleHeight(window.innerHeight - e.clientY);
    setSizeKey((n) => n + 1);
  }, []);

  const stopDrag = useCallback(() => {
    dragging.current = false;
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }, []);

  useEffect(() => {
    window.addEventListener("mousemove", onDrag);
    window.addEventListener("mouseup", stopDrag);
    return () => {
      window.removeEventListener("mousemove", onDrag);
      window.removeEventListener("mouseup", stopDrag);
    };
  }, [onDrag, stopDrag]);

  // The window changing shape changes the shell's shape.
  useEffect(() => {
    const onResize = () => setSizeKey((n) => n + 1);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    if (open) setSizeKey((n) => n + 1);
  }, [open]);

  if (!started) return null;

  return (
    <div
      aria-label="Terminal"
      className={`fixed inset-x-0 bottom-0 z-40 flex flex-col border-t border-zinc-700/70 bg-zinc-900 shadow-2xl ${
        open ? "" : "hidden"
      }`}
      style={{ height }}
    >
      <div
        role="separator"
        aria-label="Resize the terminal"
        onMouseDown={() => {
          dragging.current = true;
          document.body.style.cursor = "row-resize";
          document.body.style.userSelect = "none";
        }}
        className="h-1 w-full shrink-0 cursor-row-resize bg-zinc-800 hover:bg-emerald-500/50"
      />

      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-zinc-800 px-3 py-1.5">
        <SquareTerminal size={14} className="shrink-0 text-emerald-400" />
        <span className="text-xs font-medium text-zinc-300">Terminal</span>
        <div className="w-40 max-w-full">
          <Select
            value={account}
            onChange={setConsoleAccount}
            searchLabel="Search accounts"
            placeholder="All accounts"
            options={[
              { value: "", label: "All accounts" },
              ...accounts.map((a) => ({ value: a.id, label: a.name })),
            ]}
          />
        </div>
        <div className="w-64 max-w-full">
          <Select
            value={repo}
            onChange={setConsoleRepo}
            searchable
            searchLabel="Search repositories"
            placeholder={visible.length ? "Pick a repository" : "No repositories found"}
            options={visible.map((r) => ({
              value: r.path,
              label: account ? r.name : `${r.name}  ·  ${r.profile_name}`,
            }))}
          />
        </div>
        <span className="min-w-0 truncate text-[11px] text-zinc-500" title={repo}>
          {repo ? baseName(repo) : ""}
        </span>
        {push && (push.blocked || push.lock.locked) && (
          <span className="shrink-0" title={push.reason}>
            <Badge variant="warning">
              {currentMode(push) === "locked" ? (
                <Lock size={11} className="mr-1" />
              ) : (
                <ShieldAlert size={11} className="mr-1" />
              )}
              {currentMode(push) === "locked" ? "Locked" : "Guard-rail"}
            </Badge>
          </span>
        )}
        <div className="flex shrink-0 items-center gap-0.5 rounded-md bg-zinc-800/70 p-0.5">
          {(["terminal", "activity"] as const).map((t) => (
            <button
              key={t}
              type="button"
              aria-label={t === "terminal" ? "Shell tab" : "Activity tab"}
              onClick={() => setConsoleTab(t)}
              className={`rounded px-2 py-0.5 text-[11px] transition-colors cursor-pointer ${
                tab === t ? "bg-zinc-700 text-zinc-100" : "text-zinc-400 hover:text-zinc-200"
              }`}
            >
              {t === "terminal" ? "Shell" : "Activity"}
            </button>
          ))}
        </div>
        <button
          type="button"
          aria-label="What this terminal guards"
          onClick={() => setShowNote((v) => !v)}
          className="ml-auto flex items-center gap-1 text-[11px] text-zinc-500 hover:text-zinc-300 cursor-pointer"
        >
          <Info size={13} />
          What it guards
        </button>
        <button
          type="button"
          aria-label="Close the terminal"
          onClick={closeConsole}
          className="shrink-0 rounded p-1 text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200 cursor-pointer"
        >
          <X size={15} />
        </button>
      </div>

      {showNote && (
        <p className="shrink-0 border-b border-zinc-800 bg-zinc-800/40 px-3 py-2 text-[11px] leading-relaxed text-zinc-400">
          This shell uses the same account, SSH key and push block as your own
          terminal, because those live in your git config rather than in this
          app. On top of that, <span className="text-zinc-200">git</span> here
          refuses a force push, <span className="font-mono">--no-verify</span>,
          and a push that would skip the system config where a push lock lives.
          Those are guard-rails against a slip, not a barrier: an absolute path
          to git or a changed PATH steps around them, and your own Terminal
          always can.
        </p>
      )}

      {/* Both panels stay mounted. Unmounting the terminal would close its pty,
          so the inactive one is hidden exactly as the drawer hides itself. */}
      <div className={`min-h-0 flex-1 bg-zinc-900 px-2 py-1 ${tab === "terminal" ? "" : "hidden"}`}>
        {repo ? (
          <Suspense fallback={<p className="p-3 text-sm text-zinc-500">Starting a shell…</p>}>
            <TerminalView repoPath={repo} sizeKey={sizeKey} visible={open && tab === "terminal"} />
          </Suspense>
        ) : (
          <p className="p-3 text-sm text-zinc-500">
            Pick a repository and a shell opens in that folder.
          </p>
        )}
      </div>
      <div className={`min-h-0 flex-1 bg-zinc-900 py-1 ${tab === "activity" ? "" : "hidden"}`}>
        <ActivityView repoPath={repo} visible={open && tab === "activity"} />
      </div>
    </div>
  );
}
