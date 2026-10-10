import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Boxes, ClipboardCopy, Loader2 } from "lucide-react";
import { Button } from "../Button";
import { Input } from "../Input";
import { useToast } from "../Toast";
import * as api from "../../lib/api";

interface Props {
  /** The repository on screen — never a submodule scope. */
  repoPath: string;
  info: api.WorkspaceInfo | null;
  /** Every listener on this machine, when read; null while unread or unavailable. */
  listeners: api.Listener[] | null;
  /** The person changed the offset: the new reading, for the strip's copies row. */
  onChanged: (info: api.WorkspaceInfo) => void;
}

/** The listeners that run from this checkout: a process whose folder is inside it, or a container of its Compose project. */
export function listenersOf(rows: api.Listener[], info: api.WorkspaceInfo): api.Listener[] {
  const top = info.path.replace(/\/+$/, "");
  const inside = (p: string | null) => !!p && (p === top || p.startsWith(top + "/"));
  return rows.filter((r) => inside(r.clone?.path ?? null) || inside(r.cwd) || (!!info.compose_project && r.container?.project === info.compose_project));
}

/** The line to paste into a Terminal of one's own, so a `docker compose up` there lands in this copy's project. */
export function exportLine(info: api.WorkspaceInfo): string {
  const vars = info.env.filter(([k]) => k === "GITSWITCH_PORT_OFFSET" || k === "COMPOSE_PROJECT_NAME");
  return `export ${vars.map(([k, v]) => `${k}=${v}`).join(" ")}`;
}

/**
 * One line under the strip: which workspace this checkout is, what its
 * shells are given, and what is listening from it. The offset is the one
 * thing to change here; everything else is read.
 */
export function WorkspaceLine({ repoPath, info, listeners, onChanged }: Props) {
  const toast = useToast();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A new repository on screen: whatever was being typed is for the old one.
  useEffect(() => {
    setEditing(false);
    setError(null);
  }, [repoPath]);

  if (!info) return null;
  const mine = listeners ? listenersOf(listeners, info) : null;

  async function save() {
    const n = Number(draft.trim());
    if (!Number.isInteger(n) || n < 0 || n > 60000) {
      setError("A port offset is a whole number from 0 to 60000, usually a multiple of 100.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const next = await api.workspaceSetOffset(repoPath, n);
      onChanged(next);
      setEditing(false);
      toast.success(
        `Port offset ${n}. Servers already running keep their ports until you restart them; a Compose project started under the old name keeps its containers and volumes there.`
      );
    } catch (e) {
      setError(String(e).replace(/^Configuration error: /, ""));
    } finally {
      setSaving(false);
    }
  }

  async function copyExport() {
    const line = exportLine(info!);
    try {
      await navigator.clipboard.writeText(line);
      toast.success("Copied — paste it in your own Terminal before docker compose up");
    } catch {
      setError(`Couldn't copy — the line is ${line}`);
    }
  }

  return (
    <div className="px-1 text-xs text-zinc-400 leading-relaxed" aria-label="Workspace">
      <div className="flex items-center gap-1.5 flex-wrap">
        <Boxes size={13} className="text-zinc-500 shrink-0" />
        <span title="Exported into the terminal drawer as GITSWITCH_WORKSPACE, with the folder as GITSWITCH_WORKSPACE_PATH">
          Workspace <span className="font-mono text-zinc-300">#{info.id}</span>
          {info.kind === "worktree" && <span className="text-zinc-500"> (worktree)</span>}
        </span>
        <span className="text-zinc-600">·</span>
        {info.compose_project ? (
          <span title="COMPOSE_PROJECT_NAME in the drawer's shells: this copy's containers and volumes are named after it, apart from the first copy's">
            Compose project <span className="font-mono text-zinc-300">{info.compose_project}</span>
          </span>
        ) : (
          <span title="The first copy keeps Compose's own project name, so containers and volumes it already has stay reachable">
            Compose project: Compose&apos;s own name
          </span>
        )}
        <span className="text-zinc-600">·</span>
        {editing ? (
          <span className="inline-flex items-center gap-1.5">
            port offset
            <Input
              aria-label="Port offset"
              type="number"
              min={0}
              max={60000}
              step={100}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void save();
                if (e.key === "Escape") setEditing(false);
              }}
              className="w-24 py-0.5 text-xs"
              containerClassName="inline-block"
              autoFocus
            />
            <Button size="sm" variant="primary" disabled={saving} onClick={() => void save()} className="px-2 py-0.5">
              {saving ? <Loader2 size={12} className="animate-spin" /> : "Save"}
            </Button>
            <Button size="sm" variant="ghost" disabled={saving} onClick={() => setEditing(false)} className="px-2 py-0.5">
              Cancel
            </Button>
          </span>
        ) : (
          <span title="GITSWITCH_PORT_OFFSET in the drawer's shells — a dev server that adds it to its port stays clear of the other copy's">
            port offset <span className="font-mono text-zinc-300">{info.offset}</span>{" "}
            <button
              type="button"
              className="text-zinc-500 hover:text-zinc-300 underline-offset-2 hover:underline cursor-pointer"
              onClick={() => {
                setDraft(String(info.offset));
                setError(null);
                setEditing(true);
              }}
            >
              change
            </button>
          </span>
        )}
        <span className="text-zinc-600">·</span>
        {mine === null ? (
          <span className="text-zinc-500">listeners not read</span>
        ) : (
          <span>
            {mine.length === 0 ? "no listeners" : `${mine.length} listener${mine.length === 1 ? "" : "s"}`}{" "}
            <Link to={`/ports?clone=${encodeURIComponent(info.path)}`} className="text-sky-300 hover:underline" title="The Ports page, filtered to this checkout's folder">
              (view)
            </Link>
          </span>
        )}
        {(info.offset > 0 || info.compose_project) && (
          <>
            <span className="text-zinc-600">·</span>
            <button
              type="button"
              className="inline-flex items-center gap-1 text-zinc-500 hover:text-zinc-300 cursor-pointer"
              onClick={() => void copyExport()}
              title={`${exportLine(info)} — for a Terminal of your own: the drawer's shells already have it`}
            >
              <ClipboardCopy size={12} />
              copy export line
            </button>
          </>
        )}
      </div>
      {error && <p className="text-amber-200 mt-1 break-words">{error}</p>}
    </div>
  );
}
