import { useCallback, useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { Badge } from "../Badge";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { Select } from "../Select";
import * as api from "../../lib/api";
import type { BranchInfo, OpResult, RepoStatus } from "../../lib/api";

/** The "start from" value meaning HEAD itself, when no branch names it. */
const HEAD = "@HEAD";

/** Options for the page's `apply`. */
export type RunOptions = {
  /** A refusal with this code is a question this dialog answers itself — the page shows no toast and no result card for it. */
  quietRefusal?: string;
};

type Props = {
  open: boolean;
  onClose: () => void;
  repoPath: string;
  status: RepoStatus;
  busy: boolean;
  onResult?: (r: OpResult) => void;
  /** The page's `apply`: runs the operation, adopts the status, shows the result. */
  run: (key: string, fn: () => Promise<OpResult>, opts?: RunOptions) => Promise<OpResult | undefined>;
};

function errorText(r: OpResult): { line: string; guidance: string | null } {
  return {
    line: r.refusal?.message ?? r.advice?.headline ?? r.headline,
    guidance: r.advice?.guidance || null,
  };
}

/**
 * Local branches: switch, create, rename, delete. Nothing here touches the
 * remote. A refusal keeps the dialog open with git's reason in it — the
 * blocking files for a switch, the commit count for a delete — so the fix is
 * one look away instead of one toast that already faded.
 */
export function BranchesModal({ open, onClose, repoPath, status, busy, onResult, run }: Props) {
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<{ line: string; guidance: string | null } | null>(null);
  const [name, setName] = useState("");
  const [from, setFrom] = useState<string>(HEAD);
  const [switchTo, setSwitchTo] = useState(true);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameTo, setRenameTo] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<{ name: string; message: string } | null>(null);

  const current = status.detached || !status.branch ? HEAD : status.branch;

  const load = useCallback(() => {
    if (!repoPath) return;
    setLoading(true);
    api
      .historyBranches(repoPath)
      .then((list) => {
        setBranches(list.filter((b) => !b.is_remote));
        setLoadError(null);
      })
      .catch((e) => setLoadError(String(e)))
      .finally(() => setLoading(false));
  }, [repoPath]);

  // Every opening starts clean: the form, the errors, and a fresh list.
  useEffect(() => {
    if (!open) return;
    setName("");
    setFrom(current);
    setSwitchTo(true);
    setRenaming(null);
    setConfirmDelete(null);
    setError(null);
    load();
  }, [open, current, load]);

  const fromOptions = [
    current === HEAD
      ? { value: HEAD, label: "this commit (detached HEAD)" }
      : { value: current, label: current },
    ...branches.filter((b) => b.name !== current).map((b) => ({ value: b.name, label: b.name })),
  ];

  /** Run one action; success closes, anything else stays and explains. */
  const act = async (key: string, fn: () => Promise<OpResult>) => {
    setError(null);
    const r = await run(key, fn);
    if (!r) return undefined;
    onResult?.(r);
    if (r.ok) {
      onClose();
    } else {
      setError(errorText(r));
    }
    load();
    return r;
  };

  const create = () => {
    const n = name.trim();
    if (!n) return;
    void act("branch", () => api.changesCreateBranch(repoPath, n, from === HEAD ? null : from, switchTo));
  };

  const saveRename = (oldName: string) => {
    const n = renameTo.trim();
    if (!n || n === oldName) {
      setRenaming(null);
      return;
    }
    void act("branch", () => api.changesRenameBranch(repoPath, oldName, n)).then((r) => {
      if (r?.ok) setRenaming(null);
    });
  };

  const del = async (branch: string, force: boolean) => {
    setError(null);
    // The first, unforced attempt may come back as the `unmerged-branch`
    // question; that is this dialog's confirmation step, not a failure.
    const r = await run(
      "branch",
      () => api.changesDeleteBranch(repoPath, branch, force),
      force ? undefined : { quietRefusal: "unmerged-branch" }
    );
    if (!r) return;
    onResult?.(r);
    if (r.ok) {
      setConfirmDelete(null);
      onClose();
    } else if (!force && r.refusal?.code === "unmerged-branch") {
      // The refusal carries the count; asking again with force is the answer.
      setConfirmDelete({ name: branch, message: r.refusal.message });
    } else {
      setConfirmDelete(null);
      setError(errorText(r));
    }
    load();
  };

  return (
    <Modal open={open} onClose={onClose} title="Branches" size="lg">
      <div className="space-y-3">
        {error && (
          <div className="rounded-lg border border-red-500/30 bg-red-500/5 px-3 py-2 space-y-1">
            <p className="text-xs text-red-200 break-words">{error.line}</p>
            {error.guidance && <p className="text-xs text-red-200/80 break-words">{error.guidance}</p>}
          </div>
        )}

        <div className="rounded-lg border border-zinc-700/50 bg-zinc-800/40 p-3 space-y-2">
          <div className="flex flex-wrap items-end gap-2">
            <div className="flex-1 min-w-[12rem]">
              <label className="block text-xs text-zinc-500 mb-1">New branch</label>
              <Input
                aria-label="New branch name"
                placeholder="feature/what-it-does"
                value={name}
                onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") create();
                }}
                className="py-1.5 text-sm"
              />
            </div>
            <div className="w-56 max-w-full">
              <label className="block text-xs text-zinc-500 mb-1">Start from</label>
              <Select value={from} onChange={setFrom} options={fromOptions} />
            </div>
            <Button size="sm" disabled={busy || !name.trim()} onClick={create} className="mb-0.5">
              Create branch
            </Button>
          </div>
          <label className="flex items-center gap-2 text-xs text-zinc-300 cursor-pointer">
            <Checkbox checked={switchTo} onChange={setSwitchTo} aria-label="Switch to the new branch" />
            Switch to the new branch
          </label>
        </div>

        {confirmDelete && (
          <div className="rounded-lg border border-red-500/30 bg-red-500/5 p-3 space-y-2">
            <p className="text-sm text-red-100">
              Delete <span className="font-mono">{confirmDelete.name}</span>?
            </p>
            <p className="text-xs text-zinc-300 break-words">{confirmDelete.message}</p>
            <p className="text-xs text-zinc-500">The undo command is shown afterwards.</p>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" size="sm" disabled={busy} onClick={() => setConfirmDelete(null)}>
                Keep it
              </Button>
              <Button variant="danger" size="sm" disabled={busy} onClick={() => del(confirmDelete.name, true)}>
                Delete anyway
              </Button>
            </div>
          </div>
        )}

        {loadError && <p className="text-xs text-red-400 break-words">{loadError}</p>}

        <ul className="max-h-80 overflow-y-auto divide-y divide-zinc-800/70 rounded-lg border border-zinc-700/50">
          {loading && branches.length === 0 && (
            <li className="flex items-center gap-2 px-3 py-3 text-xs text-zinc-500">
              <Loader2 size={12} className="animate-spin" />
              Reading branches…
            </li>
          )}
          {!loading && branches.length === 0 && !loadError && (
            <li className="px-3 py-3 text-xs text-zinc-500">No local branches yet.</li>
          )}
          {branches.map((b) => (
            <li key={b.name} className="flex items-start gap-2 px-3 py-2">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="font-mono text-xs text-zinc-100">{b.name}</span>
                  {b.is_current && <Badge variant="success">HEAD</Badge>}
                  <span className="text-[11px] text-zinc-500">
                    {b.upstream ? `→ ${b.upstream}` : "not published"}
                    {b.ahead > 0 && <span className="text-emerald-400"> ↑{b.ahead}</span>}
                    {b.behind > 0 && <span className="text-sky-400"> ↓{b.behind}</span>}
                  </span>
                </div>
                <p className="text-xs text-zinc-500 truncate" title={b.last_subject}>
                  {b.last_subject}
                </p>
                {renaming === b.name && (
                  <div className="flex flex-wrap items-center gap-2 mt-1.5">
                    <Input
                      aria-label="Rename to"
                      value={renameTo}
                      onChange={(e) => setRenameTo(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") saveRename(b.name);
                        if (e.key === "Escape") setRenaming(null);
                      }}
                      containerClassName="flex-1 min-w-[10rem]"
                      className="py-1 text-xs font-mono"
                      autoFocus
                    />
                    <Button size="sm" disabled={busy || !renameTo.trim()} onClick={() => saveRename(b.name)}>
                      Save
                    </Button>
                    <Button size="sm" variant="ghost" disabled={busy} onClick={() => setRenaming(null)}>
                      Cancel
                    </Button>
                  </div>
                )}
              </div>
              <div className="flex items-center gap-1 shrink-0">
                {!b.is_current && (
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    title="Switch to this branch"
                    onClick={() => act("branch", () => api.changesSwitchBranch(repoPath, b.name))}
                  >
                    Switch
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  title="Rename this branch"
                  onClick={() => {
                    setRenaming(b.name);
                    setRenameTo(b.name);
                  }}
                >
                  Rename…
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy || b.is_current}
                  title={b.is_current ? "You can't delete the branch you're on" : "Delete this branch"}
                  className="hover:text-red-400"
                  onClick={() => del(b.name, false)}
                >
                  Delete…
                </Button>
              </div>
            </li>
          ))}
        </ul>
      </div>
    </Modal>
  );
}
