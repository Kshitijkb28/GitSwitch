import { useEffect, useRef, useState, type ReactNode } from "react";
import { GitBranch, Loader2, MessageSquare, Search as SearchIcon } from "lucide-react";
import * as api from "../../lib/api";
import { Button } from "../Button";
import { Input } from "../Input";
import { RefreshButton } from "../RefreshButton";
import { PullStateIcon, STATE_OPTIONS, Segmented, StateLabel, ago, cleanError } from "./shared";

export type PullSource = { kind: "repo"; owner: string; name: string } | { kind: "search"; scope: api.PullScope; repo: string | null };

const sourceKey = (s: PullSource) => (s.kind === "repo" ? `repo:${s.owner}/${s.name}` : `search:${s.scope}:${s.repo ?? ""}`);

/**
 * Pull requests, newest activity first: one repository's, or a search across
 * repositories. Open / Merged / Closed / All, a filter over what is loaded,
 * Refresh, and Load more. A row opens the pull request.
 */
export function PullList({
  account,
  source,
  state,
  onState,
  onOpen,
  reloadKey = 0,
  toolbar,
  onRefresh,
}: {
  account: string;
  source: PullSource;
  state: api.PullState;
  onState: (s: api.PullState) => void;
  onOpen: (item: api.PullItem) => void;
  /** Bumped by the owner to read the list again (after a merge…). */
  reloadKey?: number;
  /** More controls before the state pills (the page's scope picker…). */
  toolbar?: ReactNode;
  /** Refresh pressed: whatever else the owner read should be read again too. */
  onRefresh?: () => void;
}) {
  const [items, setItems] = useState<api.PullItem[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [total, setTotal] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  /** The request running now; an older one's answer is dropped. */
  const ask = useRef(0);

  const fetchPage = (page: number) =>
    source.kind === "repo" ? api.githubPulls(account, source.owner, source.name, state, page) : api.githubPullsSearch(account, source.scope, state, source.repo, page);

  async function load() {
    const id = ++ask.current;
    setLoading(true);
    setError(null);
    try {
      const p = await fetchPage(1);
      if (id !== ask.current) return;
      setItems(p.items);
      setNext(p.next_page);
      setTotal(p.total);
    } catch (e) {
      if (id !== ask.current) return;
      setItems([]);
      setNext(null);
      setTotal(null);
      setError(cleanError(e));
    } finally {
      if (id === ask.current) setLoading(false);
    }
  }

  async function loadMore() {
    if (!next) return;
    const id = ask.current;
    setMore(true);
    try {
      const p = await fetchPage(next);
      if (id !== ask.current) return;
      setItems((list) => [...list, ...p.items.filter((n) => !list.some((o) => o.repo === n.repo && o.number === n.number))]);
      setNext(p.next_page);
    } catch (e) {
      if (id === ask.current) setError(cleanError(e));
    } finally {
      setMore(false);
    }
  }

  useEffect(() => {
    if (account) void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account, sourceKey(source), state, reloadKey]);

  const q = filter.trim().toLowerCase();
  const shown = q
    ? items.filter((i) => [`#${i.number}`, i.title, i.author, i.head_ref ?? "", i.repo, ...i.labels].some((t) => t.toLowerCase().includes(q)))
    : items;
  const showRepo = source.kind === "search";

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-x-3 gap-y-2 flex-wrap">
        {toolbar}
        <Segmented label="Pull request state" value={state} onChange={onState} options={STATE_OPTIONS} />
        <div className="relative flex-1 min-w-[12rem]">
          <SearchIcon size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500 pointer-events-none" />
          <Input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter by title, number, author or branch" aria-label="Filter pull requests" className="!py-1.5 pl-8 text-sm" />
        </div>
        <RefreshButton
          onClick={() => {
            onRefresh?.();
            void load();
          }}
          busy={loading}
          title="Read the pull requests again"
        />
      </div>

      {error && (
        <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
          {error}
        </p>
      )}

      {loading && items.length === 0 && (
        <p className="flex items-center gap-2 text-sm text-zinc-400" role="status">
          <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading pull requests…
        </p>
      )}

      {!loading && !error && items.length === 0 && (
        <p className="text-sm text-zinc-500">
          {state === "open" ? "No open pull requests." : state === "merged" ? "No merged pull requests." : state === "closed" ? "No pull requests closed without merging." : "No pull requests."}
        </p>
      )}
      {items.length > 0 && q && shown.length === 0 && <p className="text-sm text-zinc-500">None of the {items.length} loaded match.</p>}
      {total !== null && items.length > 0 && <p className="text-xs text-zinc-500">{total.toLocaleString()} in all</p>}

      <div className="space-y-1.5">
        {shown.map((i) => (
          <button
            key={`${i.repo}#${i.number}`}
            type="button"
            data-pull-row={`${i.repo}#${i.number}`}
            onClick={() => onOpen(i)}
            className="w-full text-left flex items-start gap-3 px-3 py-2.5 rounded-lg bg-zinc-800/40 border border-zinc-700/50 hover:border-zinc-600 hover:bg-zinc-800/70 transition-colors cursor-pointer min-w-0"
          >
            <span className="mt-0.5">
              <PullStateIcon state={i.state} draft={i.draft} />
            </span>
            <span className="min-w-0 flex-1">
              <span className="flex items-center gap-2 min-w-0">
                <span className="text-zinc-100 font-medium truncate">{i.title || "(no title)"}</span>
                {i.labels.slice(0, 3).map((l) => (
                  <span key={l} className="hidden sm:inline px-1.5 py-0.5 rounded bg-zinc-700/60 text-[10px] text-zinc-300 shrink-0">
                    {l}
                  </span>
                ))}
              </span>
              <span className="block text-xs text-zinc-500 truncate">
                {showRepo && <span className="font-mono text-zinc-400">{i.repo} </span>}#{i.number} by {i.author} ·{" "}
                {i.state === "merged" ? `merged ${ago(i.merged_at)}` : i.state === "closed" ? `closed ${ago(i.closed_at)}` : `updated ${ago(i.updated_at)}`}
              </span>
              {i.head_ref && (
                <span className="flex items-center gap-1 text-xs text-zinc-500 font-mono truncate">
                  <GitBranch size={11} className="shrink-0" />
                  {i.head_ref} → {i.base_ref}
                </span>
              )}
            </span>
            <span className="flex items-center gap-2 shrink-0">
              {i.comments ? (
                <span className="flex items-center gap-1 text-xs text-zinc-500">
                  <MessageSquare size={12} />
                  {i.comments}
                </span>
              ) : null}
              <StateLabel state={i.state} draft={i.draft} />
            </span>
          </button>
        ))}
      </div>

      {next && (
        <Button variant="secondary" size="sm" onClick={() => void loadMore()} disabled={more}>
          {more && <Loader2 size={14} className="animate-spin" />}
          Load more
        </Button>
      )}
    </div>
  );
}
