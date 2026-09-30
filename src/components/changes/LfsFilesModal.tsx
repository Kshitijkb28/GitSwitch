import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Folder, FileDown, Search, Loader2 } from "lucide-react";
import { Modal } from "../Modal";
import { Button } from "../Button";
import { Input } from "../Input";
import { Checkbox } from "../Checkbox";
import * as api from "../../lib/api";
import type { LfsFile, LfsFolder, LfsListing } from "../../lib/api";

/** "2.4 GB" — the same wording the backend uses, so a size reads the same everywhere. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[u]}`;
}

/** The folder part of a path; "" at the repository root. */
function parentDir(path: string): string {
  const i = path.lastIndexOf("/");
  return i === -1 ? "" : path.slice(0, i);
}

/** Does selecting `folder` cover `path`? Prefix matching on whole segments only. */
export function covers(folder: string, path: string): boolean {
  const f = folder.replace(/\/+$/, "");
  return path === f || path.startsWith(`${f}/`);
}

/**
 * Turn a set of chosen files into the shortest list that means the same thing:
 * a folder whose every file is chosen travels as the folder.
 *
 * Normally a folder is only collapsed when the listing shows every file it
 * claims to hold, so ticking files can never pull in one nobody saw. When the
 * listing was capped that rule would make a folder undeliverable — its unlisted
 * files could never be asked for at all — so there, choosing everything the
 * list does show means the folder itself.
 */
export function compressSelection(selected: string[], listing: LfsListing): string[] {
  const chosen = new Set(selected);
  const covered = new Set<string>();
  const out: string[] = [];
  const capped = listing.truncated > 0;
  const byDepth = [...listing.folders].sort((a, b) => a.depth - b.depth || a.path.localeCompare(b.path));
  for (const folder of byDepth) {
    const under = listing.files.filter((f) => covers(folder.path, f.path));
    if (under.length === 0) continue;
    if (!capped && under.length !== folder.files) continue;
    if (under.some((f) => covered.has(f.path))) continue;
    if (under.every((f) => chosen.has(f.path))) {
      out.push(folder.path);
      under.forEach((f) => covered.add(f.path));
    }
  }
  for (const p of selected) if (!covered.has(p)) out.push(p);
  return out;
}

/** Rows rendered at once. Thousands of un-virtualised rows is the lag this
 *  feature exists to avoid, and nobody reads past a few hundred anyway. */
const MAX_ROWS = 300;

type Row =
  | { kind: "folder"; key: string; depth: number; folder: LfsFolder }
  | { kind: "file"; key: string; depth: number; file: LfsFile };

/** Folder rows in tree order, each followed by its own files when it is open. */
export function buildRows(files: LfsFile[], folders: LfsFolder[], expanded: Set<string>): Row[] {
  const shown = new Set(folders.map((f) => f.path));
  // Grouped once. Scanning every file per folder is quadratic, and a
  // repository with thousands of large files is exactly where this is used.
  const byDir = new Map<string, LfsFile[]>();
  for (const f of files) {
    const list = byDir.get(f.dir);
    if (list) list.push(f);
    else byDir.set(f.dir, [f]);
  }
  const visible = (path: string) => {
    let p = parentDir(path);
    while (p) {
      if (!expanded.has(p) || !shown.has(p)) return false;
      p = parentDir(p);
    }
    return true;
  };
  const rows: Row[] = [];
  for (const folder of [...folders].sort((a, b) => a.path.localeCompare(b.path))) {
    if (!visible(folder.path)) continue;
    rows.push({ kind: "folder", key: `d:${folder.path}`, depth: folder.depth, folder });
    if (!expanded.has(folder.path)) continue;
    for (const file of byDir.get(folder.path) ?? []) {
      rows.push({ kind: "file", key: `f:${file.path}`, depth: folder.depth + 1, file });
    }
  }
  for (const file of byDir.get("") ?? []) {
    rows.push({ kind: "file", key: `f:${file.path}`, depth: 0, file });
  }
  return rows;
}

interface Props {
  open: boolean;
  repoPath: string;
  busy: boolean;
  onClose: () => void;
  /** Download this exact selection. */
  onDownload: (paths: string[]) => void;
  /** Download every large file in the repository. */
  onDownloadAll: () => void;
}

/**
 * Browse a repository's large files and download only what is wanted.
 *
 * `git lfs pull` is all-or-nothing, which is the wrong shape for a repository
 * holding gigabytes of datasets when a person needs one folder. Sizes are shown
 * before anything is downloaded, because the whole point is deciding what is
 * worth the wait.
 */
export function LfsFilesModal({ open, repoPath, busy, onClose, onDownload, onDownloadAll }: Props) {
  const [listing, setListing] = useState<LfsListing | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [onlyMissing, setOnlyMissing] = useState(true);

  // Read the listing when the browser opens, not on every page refresh: the
  // scan walks every tracked file, which is the expensive part on a big repo.
  useEffect(() => {
    if (!open || !repoPath) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    api
      .changesLfsFiles(repoPath)
      .then((l) => {
        if (cancelled) return;
        setListing(l);
        setSelected(new Set());
        // Small trees open flat; a big one stays folded so the list is readable.
        setExpanded(new Set(l.folders.length <= 12 ? l.folders.map((f) => f.path) : []));
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, repoPath]);

  const q = query.trim().toLowerCase();
  const files = useMemo(() => {
    if (!listing) return [];
    return listing.files.filter(
      (f) => (!onlyMissing || !f.present) && (!q || f.path.toLowerCase().includes(q))
    );
  }, [listing, onlyMissing, q]);

  // Only folders that still hold something after filtering, so an empty branch
  // never sits there looking like a dead end.
  const folders = useMemo(() => {
    if (!listing) return [];
    const keep = new Set<string>();
    for (const f of files) {
      let d = f.dir;
      while (d) {
        keep.add(d);
        d = parentDir(d);
      }
    }
    return listing.folders.filter((f) => keep.has(f.path));
  }, [listing, files]);

  // A search would be useless behind folded folders.
  const effectiveExpanded = q ? new Set(folders.map((f) => f.path)) : expanded;
  const allRows = useMemo(() => buildRows(files, folders, effectiveExpanded), [files, folders, effectiveExpanded]);
  const rows = allRows.slice(0, MAX_ROWS);

  // Every folder's descendants, built in one pass up each file's ancestry, so
  // a row's checkbox state costs a lookup instead of a scan of every file.
  const under = useMemo(() => {
    const map = new Map<string, string[]>();
    for (const f of files) {
      let d = f.dir;
      while (d) {
        const list = map.get(d);
        if (list) list.push(f.path);
        else map.set(d, [f.path]);
        d = parentDir(d);
      }
    }
    return map;
  }, [files]);
  const filesUnder = (folder: string) => under.get(folder.replace(/\/+$/, "")) ?? [];
  const toggleFolder = (folder: string, on: boolean) => {
    const paths = filesUnder(folder);
    setSelected((prev) => {
      const next = new Set(prev);
      paths.forEach((p) => (on ? next.add(p) : next.delete(p)));
      return next;
    });
  };
  const toggleOpen = (folder: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(folder)) next.delete(folder);
      else next.add(folder);
      return next;
    });

  const chosen = [...selected];
  const chosenFiles = listing ? listing.files.filter((f) => selected.has(f.path)) : [];
  const chosenMissing = chosenFiles.filter((f) => !f.present);
  const chosenBytes = chosenMissing.reduce((n, f) => n + f.size, 0);

  const start = () => {
    if (!listing || chosen.length === 0) return;
    onDownload(compressSelection(chosen, listing));
    onClose();
  };

  return (
    <Modal open={open} onClose={onClose} title="Large files" size="lg">
      <div className="space-y-3">
        {loading && (
          <p className="flex items-center gap-2 text-sm text-zinc-400">
            <Loader2 size={14} className="animate-spin" /> Looking at what this repository tracks…
          </p>
        )}
        {error && <p className="text-sm text-red-400 break-words">{error}</p>}

        {listing && !loading && (
          <>
            <p className="text-sm text-zinc-300">
              {listing.total} file{listing.total === 1 ? "" : "s"}
              {listing.sizes_known ? ` · ${formatBytes(listing.total_bytes)} in total` : ""} ·{" "}
              {listing.missing === 0 ? (
                <span className="text-emerald-300">all of them are here</span>
              ) : (
                <span className="text-amber-200">
                  {listing.missing} still to download
                  {listing.sizes_known ? ` (${formatBytes(listing.missing_bytes)})` : ""}
                </span>
              )}
            </p>

            <div className="flex flex-wrap items-center gap-3">
              <div className="relative flex-1 min-w-[180px]">
                <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500" />
                <Input
                  aria-label="Search large files"
                  placeholder="Filter by name or folder"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  className="pl-8 py-1.5 text-sm"
                />
              </div>
              <label className="flex items-center gap-2 text-xs text-zinc-400 cursor-pointer">
                <Checkbox
                  aria-label="Only the ones still to download"
                  checked={onlyMissing}
                  onChange={setOnlyMissing}
                />
                Only the ones still to download
              </label>
            </div>

            <div className="max-h-[45vh] overflow-y-auto rounded-lg border border-zinc-700/50 divide-y divide-zinc-800/60">
              {rows.length === 0 && (
                <p className="p-3 text-sm text-zinc-500">
                  {q
                    ? `Nothing here matches "${query.trim()}"${onlyMissing ? " and still needs downloading" : ""}.`
                    : onlyMissing
                      ? "Every large file here is already downloaded."
                      : "This repository tracks no large files."}
                </p>
              )}
              {rows.map((row) =>
                row.kind === "folder" ? (
                  <div
                    key={row.key}
                    className="flex items-center gap-2 px-2 py-1.5 hover:bg-zinc-800/40"
                    style={{ paddingLeft: 8 + row.depth * 16 }}
                  >
                    <Checkbox
                      aria-label={`Select folder ${row.folder.path}`}
                      checked={
                        filesUnder(row.folder.path).length > 0 &&
                        filesUnder(row.folder.path).every((p) => selected.has(p))
                      }
                      onChange={(on) => toggleFolder(row.folder.path, on)}
                    />
                    <button
                      type="button"
                      onClick={() => toggleOpen(row.folder.path)}
                      aria-label={`Toggle folder ${row.folder.path}`}
                      className="flex min-w-0 flex-1 items-center gap-1.5 text-left cursor-pointer"
                    >
                      {effectiveExpanded.has(row.folder.path) ? (
                        <ChevronDown size={13} className="shrink-0 text-zinc-500" />
                      ) : (
                        <ChevronRight size={13} className="shrink-0 text-zinc-500" />
                      )}
                      <Folder size={13} className="shrink-0 text-zinc-500" />
                      <span className="truncate text-sm text-zinc-200">
                        {row.folder.path.slice(parentDir(row.folder.path).length ? parentDir(row.folder.path).length + 1 : 0)}
                      </span>
                    </button>
                    <span className="shrink-0 text-xs text-zinc-500">
                      {row.folder.missing > 0
                        ? `${row.folder.missing} of ${row.folder.files} to download${
                            listing.sizes_known ? ` · ${formatBytes(row.folder.missing_bytes)}` : ""
                          }`
                        : `${row.folder.files} file${row.folder.files === 1 ? "" : "s"}${
                            listing.sizes_known ? ` · ${formatBytes(row.folder.bytes)}` : ""
                          }`}
                    </span>
                  </div>
                ) : (
                  <div
                    key={row.key}
                    className="flex items-center gap-2 px-2 py-1.5 hover:bg-zinc-800/40"
                    style={{ paddingLeft: 8 + row.depth * 16 }}
                  >
                    <Checkbox
                      aria-label={`Select file ${row.file.path}`}
                      checked={selected.has(row.file.path)}
                      onChange={(on) =>
                        setSelected((prev) => {
                          const next = new Set(prev);
                          if (on) next.add(row.file.path);
                          else next.delete(row.file.path);
                          return next;
                        })
                      }
                    />
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-zinc-300" title={row.file.path}>
                      {row.file.path.slice(row.file.dir ? row.file.dir.length + 1 : 0)}
                    </span>
                    <span className="shrink-0 text-xs text-zinc-500">
                      {listing.sizes_known ? formatBytes(row.file.size) : "size unknown"}
                    </span>
                    <span
                      className={`shrink-0 text-xs ${row.file.present ? "text-emerald-400/80" : "text-amber-300/80"}`}
                    >
                      {row.file.present ? "here" : row.file.downloaded ? "downloaded, not written out" : "to download"}
                    </span>
                  </div>
                )
              )}
            </div>

            {allRows.length > rows.length && (
              <p className="text-xs text-zinc-500">
                Showing the first {rows.length} of {allRows.length} rows. Narrow the filter, or fold a folder,
                to reach the rest — a folder can be chosen whole without opening it.
              </p>
            )}
            {listing.folders_truncated > 0 && (
              <p className="text-xs text-zinc-500">
                {listing.folders_truncated} deeper folder{listing.folders_truncated === 1 ? " is" : "s are"} not
                listed. Their files are counted in the folders above them, which can be chosen whole.
              </p>
            )}
            {listing.truncated > 0 && (
              <p className="text-xs text-zinc-500">
                {listing.truncated} more file{listing.truncated === 1 ? "" : "s"} aren't listed here. The folder
                counts do include them, and choosing a whole folder downloads all of it — listed or not.
              </p>
            )}

            <div className="flex flex-wrap items-center gap-2 pt-1">
              <p className="mr-auto text-sm text-zinc-300">
                {chosen.length === 0
                  ? "Nothing selected yet."
                  : `${chosen.length} selected · ${chosenMissing.length} to download${
                      listing.sizes_known ? ` (${formatBytes(chosenBytes)})` : ""
                    }`}
              </p>
              <Button size="sm" variant="ghost" onClick={onClose} disabled={busy}>
                Close
              </Button>
              <Button
                size="sm"
                variant="secondary"
                onClick={() => {
                  onDownloadAll();
                  onClose();
                }}
                disabled={busy || listing.missing === 0}
                title={
                  listing.missing === 0
                    ? "Every large file is already here"
                    : `Downloads all ${listing.missing} missing files${
                        listing.sizes_known ? ` (${formatBytes(listing.missing_bytes)})` : ""
                      }`
                }
              >
                Download everything
              </Button>
              <Button
                size="sm"
                variant="primary"
                onClick={start}
                disabled={busy || chosenMissing.length === 0}
                title={
                  chosenMissing.length === 0
                    ? "Pick a file or a folder that still needs downloading"
                    : "Downloads only what is selected"
                }
              >
                <FileDown size={13} />
                Download selected
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}
