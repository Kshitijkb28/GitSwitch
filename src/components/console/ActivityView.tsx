import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, ChevronRight, Copy, Eraser } from "lucide-react";
import { Checkbox } from "../Checkbox";
import * as api from "../../lib/api";
import { copyText } from "../../lib/clipboard";

interface Props {
  /** Only this repository's commands. "" means every repository. */
  repoPath: string;
  /** Poll only while the tab is actually on screen. */
  visible: boolean;
}

/** "1.2s" / "340ms" — a duration you read rather than parse. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms / 1000)}s`;
}

/** The command as you would type it, quoting only what needs it. */
export function commandLine(argv: string[]): string {
  return argv
    .map((a) => (/[\s'"\\]/.test(a) ? `'${a.replace(/'/g, `'\\''`)}'` : a))
    .join(" ");
}

/**
 * What the app itself ran.
 *
 * The terminal next door shows a shell, and a shell only knows what you typed.
 * This is the other half: every git command GitSwitch ran on your behalf, with
 * its exit code, how long it took and what git said. Routine reads are hidden
 * by default because they are the overwhelming majority and none of them
 * changed anything.
 */
export function ActivityView({ repoPath, visible }: Props) {
  const [entries, setEntries] = useState<api.ConsoleEntry[]>([]);
  const [hideReads, setHideReads] = useState(true);
  const [open, setOpen] = useState<Set<number>>(new Set());
  const [copied, setCopied] = useState<number | null>(null);
  const [allRepos, setAllRepos] = useState(false);
  const since = useRef(0);
  const scroller = useRef<HTMLDivElement>(null);

  // Changing the filter means the ids already seen no longer apply. This
  // runs BEFORE the poll below (effects run in order): the first read of the
  // new filter must start from zero, not from the old filter's last id, or
  // everything that repository ran before now is never shown.
  useEffect(() => {
    since.current = 0;
    setEntries([]);
    setOpen(new Set());
  }, [repoPath, allRepos]);

  // Same shape as the LFS progress poll: an immediate first read, a flag so a
  // late promise cannot set state after the tab closed, and a silent catch
  // because a log is never allowed to raise an error of its own.
  useEffect(() => {
    if (!visible) return;
    let stop = false;
    const read = () =>
      api
        .consoleEntries(since.current, allRepos ? "" : repoPath)
        .then((fresh) => {
          if (stop || fresh.length === 0) return;
          since.current = fresh[fresh.length - 1].id;
          setEntries((prev) => [...prev, ...fresh].slice(-1000));
        })
        .catch(() => {
          /* the backend may be mid-restart; the next tick tries again */
        });
    read();
    const id = window.setInterval(read, 1000);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [visible, repoPath, allRepos]);

  const shown = useMemo(
    () => (hideReads ? entries.filter((e) => !e.is_read) : entries),
    [entries, hideReads]
  );

  // Follow the tail, but only when already at the bottom, so reading an older
  // entry is not yanked away by the next status refresh.
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    if (atBottom) el.scrollTop = el.scrollHeight;
  }, [shown.length]);

  const copy = useCallback(async (e: api.ConsoleEntry) => {
    if (await copyText(commandLine(e.argv))) {
      setCopied(e.id);
      window.setTimeout(() => setCopied((c) => (c === e.id ? null : c)), 1200);
    }
  }, []);

  const clear = useCallback(() => {
    void api.consoleClear().catch(() => {});
    since.current = 0;
    setEntries([]);
    setOpen(new Set());
  }, []);

  return (
    <div className="flex h-full min-h-0 w-full flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-3 px-2 pb-1.5 text-[11px] text-zinc-500">
        <label className="flex items-center gap-1.5 cursor-pointer">
          <Checkbox
            aria-label="Hide routine reads"
            checked={hideReads}
            onChange={setHideReads}
          />
          Hide routine reads
        </label>
        <label className="flex items-center gap-1.5 cursor-pointer">
          <Checkbox
            aria-label="Every repository"
            checked={allRepos}
            onChange={setAllRepos}
          />
          Every repository
        </label>
        <span className="text-zinc-600">
          {shown.length} of {entries.length} shown
        </span>
        <button
          type="button"
          onClick={clear}
          className="ml-auto flex items-center gap-1 text-zinc-500 hover:text-zinc-300 cursor-pointer"
        >
          <Eraser size={12} />
          Clear
        </button>
      </div>

      <div ref={scroller} className="min-h-0 flex-1 overflow-y-auto">
        {shown.length === 0 ? (
          <p className="px-3 py-2 text-xs text-zinc-500">
            {entries.length === 0
              ? "Nothing yet. Anything the app asks git to do shows up here."
              : "Only routine reads so far — untick the box to see them."}
          </p>
        ) : (
          shown.map((e) => {
            const failed = e.exit_code !== 0;
            const expanded = open.has(e.id);
            const body = [e.stdout, e.stderr].filter(Boolean).join("\n");
            return (
              <div key={e.id} className="border-b border-zinc-800/60">
                <div className="flex items-center gap-2 px-2 py-1 hover:bg-zinc-800/40">
                  <button
                    type="button"
                    aria-label={`Toggle ${commandLine(e.argv)}`}
                    onClick={() =>
                      setOpen((prev) => {
                        const next = new Set(prev);
                        if (next.has(e.id)) next.delete(e.id);
                        else next.add(e.id);
                        return next;
                      })
                    }
                    className="flex min-w-0 flex-1 items-center gap-1.5 text-left cursor-pointer"
                    title={commandLine(e.argv)}
                  >
                    {body ? (
                      expanded ? (
                        <ChevronDown size={12} className="shrink-0 text-zinc-600" />
                      ) : (
                        <ChevronRight size={12} className="shrink-0 text-zinc-600" />
                      )
                    ) : (
                      <span className="w-3 shrink-0" />
                    )}
                    <span
                      className={`min-w-0 truncate font-mono text-[11px] ${
                        failed ? "text-red-300" : e.is_read ? "text-zinc-500" : "text-zinc-200"
                      }`}
                    >
                      $ {commandLine(e.argv)}
                    </span>
                  </button>
                  <span
                    className={`shrink-0 text-[10px] ${failed ? "text-red-400" : "text-emerald-400/70"}`}
                  >
                    {failed ? `exit ${e.exit_code}` : "ok"}
                  </span>
                  <span className="shrink-0 text-[10px] text-zinc-600">
                    {formatDuration(e.duration_ms)}
                  </span>
                  <button
                    type="button"
                    aria-label={`Copy ${commandLine(e.argv)}`}
                    onClick={() => void copy(e)}
                    className="shrink-0 rounded p-0.5 text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300 cursor-pointer"
                  >
                    {copied === e.id ? <Check size={12} className="text-emerald-400" /> : <Copy size={12} />}
                  </button>
                </div>
                {expanded && body && (
                  <pre className="whitespace-pre-wrap break-all px-6 pb-2 text-[11px] leading-relaxed text-zinc-400">
                    {body}
                    {e.truncated && (
                      <span className="text-amber-400/90">{"\n… output was longer and was cut."}</span>
                    )}
                  </pre>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
