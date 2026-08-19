import { useState } from "react";
import {
  GitBranch,
  FolderOpen,
  Loader2,
  CheckCircle2,
  Download,
  Folder,
  RefreshCw,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Checkbox } from "../components/Checkbox";
import { Input } from "../components/Input";
import { Badge } from "../components/Badge";
import { useToast } from "../components/Toast";
import { baseName } from "../lib/paths";
import * as api from "../lib/api";

export function SparseClone() {
  const toast = useToast();
  const [url, setUrl] = useState("");
  const [parentDir, setParentDir] = useState("");
  const [folderName, setFolderName] = useState("");
  const [cloning, setCloning] = useState(false);
  const [info, setInfo] = useState<api.SparseInfo | null>(null);
  const [loadingInfo, setLoadingInfo] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function loadInfo(repoPath: string) {
    setLoadingInfo(true);
    setError(null);
    try {
      const i = await api.sparseRepoInfo(repoPath);
      setInfo(i);
      setSelected(new Set(i.sparse_dirs));
    } catch (e) {
      setError(String(e));
      setInfo(null);
    } finally {
      setLoadingInfo(false);
    }
  }

  async function browseParent() {
    const dir = await open({ directory: true, multiple: false, title: "Where should the repo be cloned?" });
    if (typeof dir === "string") setParentDir(dir);
  }

  async function browseExisting() {
    const dir = await open({ directory: true, multiple: false, title: "Select an existing repo folder" });
    if (typeof dir === "string") await loadInfo(dir);
  }

  async function handleClone() {
    if (!url.trim() || !parentDir) {
      setError("Enter a repo URL and choose a destination folder.");
      return;
    }
    setCloning(true);
    setError(null);
    try {
      const repoPath = await api.sparseClone(url, parentDir, folderName || null);
      toast.success("Repository cloned (metadata only) — now pick folders");
      await loadInfo(repoPath);
    } catch (e) {
      setError(String(e));
    } finally {
      setCloning(false);
    }
  }

  function toggle(dir: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(dir)) next.delete(dir);
      else next.add(dir);
      return next;
    });
  }

  async function handleApply() {
    if (!info) return;
    setApplying(true);
    setError(null);
    try {
      await api.sparseSet(info.path, Array.from(selected));
      toast.success(
        `Checked out ${selected.size} folder${selected.size === 1 ? "" : "s"}`
      );
      await loadInfo(info.path);
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-zinc-100">Sparse Clone</h1>
        <p className="text-sm text-zinc-400 mt-1">
          Clone only the folders you need from a big repo — fast, small, and
          you can add more folders anytime.
        </p>
      </div>

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      <Card>
        <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-1">
          Clone a new repo
        </h2>
        <p className="text-xs text-zinc-500 mb-4">
          Metadata-only clone (<span className="font-mono">--filter=blob:none --no-checkout</span>)
          — no files download until you pick folders below. Use an SSH URL
          (<span className="font-mono">git@github.com:…</span>) so your per-folder
          account applies.
        </p>
        <div className="space-y-3">
          <Input
            label="Repository URL"
            placeholder="git@github.com:Ethara-Ai/unified-personas.git"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <div className="flex gap-2 items-end">
            <Input
              label="Clone into"
              placeholder="Choose a destination folder…"
              value={parentDir}
              onChange={(e) => setParentDir(e.target.value)}
              className="flex-1"
            />
            <Button type="button" variant="secondary" onClick={browseParent}>
              <FolderOpen size={16} />
              Browse
            </Button>
          </div>
          <Input
            label="Folder name (optional)"
            placeholder="Defaults to the repo name"
            value={folderName}
            onChange={(e) => setFolderName(e.target.value)}
          />
          <div className="flex items-center gap-3">
            <Button onClick={handleClone} disabled={cloning}>
              {cloning ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  Cloning…
                </>
              ) : (
                <>
                  <Download size={16} />
                  Sparse Clone
                </>
              )}
            </Button>
            <span className="text-xs text-zinc-500">
              …or manage an already-cloned repo:
            </span>
            <Button type="button" variant="ghost" size="sm" onClick={browseExisting}>
              <FolderOpen size={14} />
              Open existing repo
            </Button>
          </div>
        </div>
      </Card>

      {loadingInfo && (
        <Card>
          <div className="flex items-center justify-center gap-2 py-6 text-sm text-zinc-400">
            <Loader2 size={16} className="animate-spin text-emerald-400" />
            Reading repository folders…
          </div>
        </Card>
      )}

      {info && !loadingInfo && (
        <Card>
          <div className="flex items-center justify-between mb-1 gap-3 flex-wrap">
            <div className="flex items-center gap-2 min-w-0">
              <GitBranch size={16} className="text-emerald-400 shrink-0" />
              <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider truncate">
                {baseName(info.path)}
              </h2>
              <Badge variant="success">{info.branch}</Badge>
              {info.is_sparse && <Badge variant="default">sparse</Badge>}
            </div>
            <button
              onClick={() => loadInfo(info.path)}
              title="Re-read folders"
              className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800 transition-colors cursor-pointer"
            >
              <RefreshCw size={13} />
              Refresh
            </button>
          </div>
          <p className="text-xs text-zinc-500 font-mono mb-4 truncate">{info.path}</p>

          {info.available_dirs.length === 0 ? (
            <p className="text-sm text-zinc-500">
              No top-level folders found in this repo.
            </p>
          ) : (
            <>
              <p className="text-xs text-zinc-500 mb-2">
                Tick the folders to materialize — {selected.size} of{" "}
                {info.available_dirs.length} selected
              </p>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-1.5 mb-4 max-h-72 overflow-y-auto pr-1">
                {info.available_dirs.map((dir) => {
                  const checked = selected.has(dir);
                  const live = info.sparse_dirs.includes(dir);
                  return (
                    <label
                      key={dir}
                      className={`flex items-center gap-2.5 px-3 py-2 rounded-lg border cursor-pointer transition-colors ${
                        checked
                          ? "bg-emerald-500/10 border-emerald-500/40"
                          : "bg-zinc-800/50 border-zinc-700/50 hover:border-zinc-600"
                      }`}
                    >
                      <Checkbox checked={checked} onChange={() => toggle(dir)} />
                      <Folder
                        size={14}
                        className={checked ? "text-emerald-400" : "text-zinc-500"}
                      />
                      <span className="text-sm text-zinc-200 font-mono truncate flex-1">
                        {dir}
                      </span>
                      {live && (
                        <CheckCircle2
                          size={14}
                          className="text-emerald-400 shrink-0"
                          aria-label="currently checked out"
                        />
                      )}
                    </label>
                  );
                })}
              </div>
              <div className="flex items-center justify-between">
                <p className="text-xs text-zinc-600">
                  Equivalent to{" "}
                  <span className="font-mono text-zinc-500">
                    git sparse-checkout set {Array.from(selected).join(" ") || "…"}
                  </span>
                </p>
                <Button onClick={handleApply} disabled={applying || selected.size === 0}>
                  {applying ? (
                    <>
                      <Loader2 size={16} className="animate-spin" />
                      Checking out…
                    </>
                  ) : (
                    "Apply selection"
                  )}
                </Button>
              </div>
            </>
          )}
        </Card>
      )}
    </div>
  );
}
