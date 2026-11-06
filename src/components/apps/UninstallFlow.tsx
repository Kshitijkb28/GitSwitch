import { useEffect, useRef, useState } from "react";
import { Trash2, Loader2, FolderOpen, Copy, Check, AlertTriangle, ExternalLink, Shield } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Modal } from "../Modal";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { formatBytes } from "../changes/LfsFilesModal";
import { cleanError } from "../github/shared";
import * as api from "../../lib/api";

type Stage = "preparing" | "confirm" | "running" | "forcing" | "gone" | "still" | "failed" | "scanning" | "leftovers" | "deleting" | "done";

function Lines({ items, className = "text-zinc-400" }: { items: string[]; className?: string }) {
  if (items.length === 0) return null;
  return (
    <ul className={`pl-4 space-y-0.5 ${className}`}>
      {items.map((l) => (
        <li key={l} className="list-disc break-words">
          {l}
        </li>
      ))}
    </ul>
  );
}

function CopyCommand({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex items-start gap-2">
      <pre className="flex-1 min-w-0 whitespace-pre-wrap break-all rounded-lg border border-zinc-700 bg-zinc-900 px-2 py-1.5 font-mono text-[11px] text-zinc-300">{command}</pre>
      <button
        type="button"
        title="Copy"
        className="p-1.5 rounded-md text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800 cursor-pointer"
        onClick={() => {
          navigator.clipboard?.writeText(command).then(() => setCopied(true), () => {});
        }}
      >
        {copied ? <Check size={13} /> : <Copy size={13} />}
      </button>
    </div>
  );
}

/** What GitSwitch did with a leftover, in words. */
const OUTCOME: Record<string, [string, string]> = {
  trashed: ["Moved to the Trash", "text-emerald-400"],
  removed: ["Removed", "text-emerald-400"],
  gone: ["Already gone", "text-zinc-400"],
  changed: ["Kept — it changed", "text-amber-300"],
  refused: ["Kept", "text-amber-300"],
  "needs-admin": ["Needs the administrator", "text-amber-300"],
  protected: ["Protected by the system", "text-amber-300"],
  failed: ["Failed", "text-red-400"],
};

/** Leftovers that are not files: nothing to show in a folder. */
const NOT_FILES = ["registry", "package-settings", "startup"];

/**
 * One uninstall, step by step: what will happen and what is known about the
 * app; the uninstall (or, for a Windows program, deleting its files
 * directly); then only its leftovers, ticked when sure, to the Trash — on
 * Windows, what needs the administrator after one permission prompt.
 * The page sends the backend ids only — never a path.
 */
export function UninstallFlow({
  app,
  platform,
  resume,
  onClose,
  onChanged,
}: {
  app: api.InstalledApp | null;
  platform: api.AppList["platform"];
  /** An unfinished uninstall to pick up at "look for leftovers". */
  resume?: { session: string; name: string } | null;
  onClose: () => void;
  onChanged: () => void;
}) {
  const open = !!app || !!resume;
  const [stage, setStage] = useState<Stage>("preparing");
  const [prep, setPrep] = useState<api.AppPrepared | null>(null);
  const [session, setSession] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [alsoOk, setAlsoOk] = useState(false);
  const [registry, setRegistry] = useState(false);
  const [forceOpen, setForceOpen] = useState(false);
  const [scan, setScan] = useState<api.LeftoverScan | null>(null);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [results, setResults] = useState<api.LeftoverDeleted[]>([]);
  const alive = useRef(0);
  const name = app?.name ?? resume?.name ?? "";

  useEffect(() => {
    if (!open) return;
    const run = ++alive.current;
    setPrep(null);
    setError(null);
    setMessage(null);
    setScan(null);
    setResults([]);
    setAlsoOk(false);
    setRegistry(false);
    setForceOpen(false);
    if (resume) {
      setSession(resume.session);
      setStage("gone");
      return;
    }
    setSession(null);
    setStage("preparing");
    api.appsPrepare(app!.id).then(
      (p) => {
        if (alive.current !== run) return;
        setPrep(p);
        setSession(p.session);
        setStage("confirm");
      },
      (e) => {
        if (alive.current !== run) return;
        setError(cleanError(e));
        setStage("failed");
      }
    );
    return () => {
      alive.current++;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [app?.id, resume?.session, open]);

  // While an uninstaller runs (Windows), follow it.
  useEffect(() => {
    if (stage !== "running" || !session) return;
    const run = alive.current;
    const t = window.setInterval(() => {
      api.appsStatus(session).then(
        (p) => {
          if (alive.current !== run) return;
          follow(p);
        },
        () => {}
      );
    }, 1500);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stage, session]);

  function follow(p: api.AppProgress) {
    setMessage(p.message);
    if (p.state === "gone") {
      setStage("gone");
      onChanged();
    } else if (p.state === "still-installed") setStage("still");
    else if (p.state === "failed") setStage("failed");
    else setStage("running");
  }

  async function uninstall() {
    if (!session) return;
    setError(null);
    setForceOpen(false);
    setStage("running");
    try {
      follow(await api.appsUninstall(session));
    } catch (e) {
      setError(cleanError(e));
      setStage("failed");
    }
  }

  /** Windows: its folders to the Recycle Bin, its entry and start-up values removed. */
  async function forceRemove() {
    if (!session) return;
    const back = stage;
    setError(null);
    setStage("forcing");
    try {
      const p = await api.appsForceRemove(session);
      setForceOpen(false);
      follow(p);
    } catch (e) {
      // Nothing was done (the permission prompt was declined, say): back where it was.
      setError(cleanError(e));
      setStage(back);
    }
  }

  async function checkGone() {
    if (!session) return;
    setError(null);
    try {
      follow(await api.appsCheckGone(session));
    } catch (e) {
      setError(cleanError(e));
    }
  }

  async function lookForLeftovers() {
    if (!session) return;
    setError(null);
    setStage("scanning");
    try {
      const s = await api.appsScan(session, platform === "windows" && registry);
      setScan(s);
      setTicked(new Set(s.items.filter((i) => i.confidence === "high" && (!i.admin || i.elevate)).map((i) => i.id)));
      setStage("leftovers");
    } catch (e) {
      setError(cleanError(e));
      setStage("still");
    }
  }

  async function deleteTicked() {
    if (!scan) return;
    setError(null);
    setStage("deleting");
    try {
      setResults(await api.appsDelete(scan.scan, [...ticked]));
      setStage("done");
    } catch (e) {
      setError(cleanError(e));
      setStage("leftovers");
    }
  }

  /** The ones Windows' permission prompt was declined for, again. */
  async function retryAdmin(ids: string[]) {
    if (!scan) return;
    setError(null);
    setStage("deleting");
    try {
      const again = await api.appsDelete(scan.scan, ids);
      setResults((prev) => prev.map((r) => again.find((a) => a.id === r.id) ?? r));
    } catch (e) {
      setError(cleanError(e));
    }
    setStage("done");
  }

  const busy = stage === "preparing" || stage === "scanning" || stage === "deleting" || stage === "forcing";
  const trashWord = platform === "windows" ? "Recycle Bin" : "Trash";
  const tickable = (i: api.Leftover) => !i.admin || !!i.elevate;
  const deletable = scan?.items.filter(tickable) ?? [];
  const adminItems = scan?.items.filter((i) => !tickable(i)) ?? [];
  const places = [...new Set(deletable.map((i) => i.place))];
  const tickedItems = deletable.filter((i) => ticked.has(i.id));
  const tickedSize = tickedItems.reduce((n, i) => n + (i.size ?? 0), 0);
  const tickedAdmin = tickedItems.filter((i) => i.admin).length;
  const retryable = results.filter((r) => r.outcome === "needs-admin" && scan?.items.find((i) => i.id === r.id)?.elevate);
  const force = platform === "windows" ? prep?.force ?? null : null;

  const forcePanel = force && forceOpen && (
    <div className="rounded-lg border border-red-500/30 bg-red-500/5 px-3 py-2 space-y-1.5" data-flow-force>
      <p className="text-zinc-200">Without running its uninstaller, GitSwitch:</p>
      <ul className="pl-4 space-y-0.5 text-zinc-300">
        {force.folders.length === 0 && (
          <li className="list-disc break-words" data-force-no-files>
            finds its files already gone — nothing to move
          </li>
        )}
        {force.folders.map((f) => (
          <li key={f.path} className="list-disc break-words" data-force-folder>
            moves <span className="font-mono text-[11px]">{f.path}</span>
            {f.size != null ? ` (${formatBytes(f.size)}${f.size_complete ? "" : "+"})` : ""} to the Recycle Bin
          </li>
        ))}
        <li className="list-disc break-words" title={force.entry}>
          removes it from Installed apps (its entry is backed up first)
        </li>
        {force.startup.map((s) => (
          <li key={s} className="list-disc break-words">
            removes its start-up entry {s}
          </li>
        ))}
      </ul>
      {force.folders.length > 0 && (
        <p className="text-amber-300/90">Everything in that folder goes with it, including anything you saved there; it can be restored from the Recycle Bin.</p>
      )}
      <p className="text-zinc-500">Its uninstaller is not run, so whatever it set up elsewhere in Windows stays — Look for leftovers afterwards finds the rest.</p>
      {force.admin && (
        <p className="flex items-center gap-1.5 text-zinc-300" data-force-admin>
          <Shield size={12} /> Windows asks for permission.
        </p>
      )}
      <div className="flex items-center gap-2 pt-1">
        <Button variant="danger" size="sm" onClick={forceRemove} disabled={!!prep?.running} data-flow-force-go>
          <Trash2 size={14} /> Delete them
        </Button>
        <Button variant="ghost" size="sm" onClick={() => setForceOpen(false)}>
          Cancel
        </Button>
      </div>
    </div>
  );

  const forceButton = force && !forceOpen && (
    <Button variant="secondary" size="sm" onClick={() => setForceOpen(true)} disabled={!!prep?.blocked} data-flow-force-open>
      <Trash2 size={14} /> Delete its files instead
    </Button>
  );

  return (
    <Modal open={open} onClose={stage === "running" || busy ? () => {} : onClose} title={`Uninstall ${name}`} size="lg">
      <div className="max-h-[72vh] overflow-y-auto pr-1 space-y-3 text-xs text-zinc-400" data-flow-stage={stage}>
        {error && (
          <p className="whitespace-pre-wrap break-words rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-red-400" data-flow-error>
            {error}
          </p>
        )}
        {busy && (
          <p className="flex items-center gap-2 text-zinc-300">
            <Loader2 size={14} className="animate-spin" />
            {stage === "preparing"
              ? `Reading what identifies ${name}…`
              : stage === "scanning"
                ? "Looking for what it left behind…"
                : stage === "forcing"
                  ? `Deleting ${name}'s files…`
                  : `Moving them to the ${trashWord}…`}
          </p>
        )}

        {stage === "confirm" && prep && (
          <>
            <p className="text-zinc-300">{prep.method}</p>
            {prep.command && <CopyCommand command={prep.command} />}
            {prep.blocked && (
              <p className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-red-300" data-flow-blocked>
                <AlertTriangle size={14} className="mt-0.5 shrink-0" /> {prep.blocked}
              </p>
            )}
            {prep.running && <p className="text-amber-300">{name} is running. Quit it first.</p>}
            <Lines items={prep.warnings} className="text-amber-300/90" />
            {prep.also_removed.length > 0 && (
              <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 px-3 py-2 space-y-1.5" data-flow-also>
                <p className="text-amber-200">Removing it also removes {prep.also_removed.length} other package{prep.also_removed.length === 1 ? "" : "s"}:</p>
                <p className="font-mono text-[11px] text-zinc-300 break-words">{prep.also_removed.join("  ")}</p>
                <label className="flex items-center gap-2 cursor-pointer text-zinc-300">
                  <Checkbox checked={alsoOk} onChange={setAlsoOk} aria-label="Remove them too" />
                  Remove them too
                </label>
              </div>
            )}
            <details className="text-zinc-500">
              <summary className="cursor-pointer select-none">What identifies it (used to find only its leftovers)</summary>
              <Lines items={prep.facts} />
            </details>
            {forcePanel}
            <div className="flex items-center gap-2 flex-wrap pt-1">
              <Button
                variant="danger"
                size="sm"
                onClick={uninstall}
                disabled={!!prep.blocked || prep.running || (prep.also_removed.length > 0 && !alsoOk)}
                data-flow-go
              >
                <Trash2 size={14} /> Uninstall
              </Button>
              {forceButton}
              {prep.vendor_uninstaller && (
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    api.appsOpenUninstaller(prep.session).then(() => setStage("still"), (e) => setError(cleanError(e)));
                    setMessage(`${name}'s own uninstaller is open. When it has finished, check here.`);
                  }}
                >
                  <ExternalLink size={14} /> Use its own uninstaller
                </Button>
              )}
              <Button variant="ghost" size="sm" onClick={onClose}>
                Cancel
              </Button>
            </div>
            {platform === "windows" && !force && prep.force_unavailable && (
              <p className="text-zinc-500" data-flow-force-unavailable>
                Deleting its files directly isn't offered: {prep.force_unavailable}
              </p>
            )}
          </>
        )}

        {stage === "running" && (
          <div className="space-y-2" data-flow-running>
            <p className="flex items-center gap-2 text-zinc-300">
              <Loader2 size={14} className="animate-spin" /> {message ?? `Uninstalling ${name}…`}
            </p>
            {platform === "windows" && (
              <Button variant="secondary" size="sm" onClick={checkGone}>
                It's finished — check
              </Button>
            )}
          </div>
        )}

        {(stage === "still" || stage === "failed") && session && (
          <div className="space-y-2" data-flow-still>
            {message && <p className="text-amber-300 whitespace-pre-wrap">{message}</p>}
            {stage === "still" && <p>Its files are still its own while it is installed, so nothing else is looked at yet.</p>}
            {forcePanel}
            <div className="flex items-center gap-2 flex-wrap">
              <Button variant="secondary" size="sm" onClick={checkGone}>
                It's gone — check again
              </Button>
              {stage === "failed" && prep && !prep.blocked && (
                <Button variant="ghost" size="sm" onClick={uninstall}>
                  Try again
                </Button>
              )}
              {forceButton}
              <Button variant="ghost" size="sm" onClick={onClose}>
                Close
              </Button>
            </div>
          </div>
        )}

        {stage === "gone" && (
          <div className="space-y-2" data-flow-gone>
            <p className="text-emerald-400">{message ?? `${name} is uninstalled.`}</p>
            <p>GitSwitch can now look for what it left behind — only files named after this app, in the places apps keep their data.</p>
            {platform === "windows" && (
              <label className="flex items-center gap-2 cursor-pointer text-zinc-300" data-flow-registry>
                <Checkbox checked={registry} onChange={setRegistry} aria-label="Also look in the registry" />
                Also look in the registry (your own settings under HKCU; a backup is saved before anything is deleted)
              </label>
            )}
            <div className="flex items-center gap-2">
              <Button size="sm" onClick={lookForLeftovers} data-flow-scan>
                Look for leftovers
              </Button>
              <Button variant="ghost" size="sm" onClick={onClose}>
                Done
              </Button>
            </div>
          </div>
        )}

        {stage === "leftovers" && scan && (
          <div className="space-y-3" data-flow-leftovers>
            {scan.items.length === 0 && <p className="text-zinc-300">Nothing of {name}'s was left behind in the places apps keep their data.</p>}
            {deletable.length > 1 && (
              <button
                type="button"
                className="text-[11px] text-emerald-400 hover:underline cursor-pointer"
                onClick={() => setTicked(ticked.size === deletable.length ? new Set() : new Set(deletable.map((i) => i.id)))}
                data-flow-tick-all
              >
                {ticked.size === deletable.length ? "Untick all" : "Tick all"}
              </button>
            )}
            {places.map((place) => (
              <section key={place}>
                <h3 className="text-[11px] font-semibold uppercase tracking-wider text-zinc-300 mb-1">{place}</h3>
                <ul className="space-y-1">
                  {deletable
                    .filter((i) => i.place === place)
                    .map((i) => (
                      <li key={i.id} className="flex items-start gap-2" data-leftover={i.id} data-confidence={i.confidence}>
                        <Checkbox
                          checked={ticked.has(i.id)}
                          onChange={(v) =>
                            setTicked((t) => {
                              const n = new Set(t);
                              if (v) n.add(i.id);
                              else n.delete(i.id);
                              return n;
                            })
                          }
                          aria-label={`Delete ${i.path}`}
                        />
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2 min-w-0">
                            <span className="font-mono text-[11px] text-zinc-300 truncate" title={i.path}>
                              {i.path.split(/[\\/]/).pop()}
                            </span>
                            <span
                              className={`shrink-0 px-1.5 rounded border text-[10px] ${i.confidence === "high" ? "border-emerald-500/40 text-emerald-300" : "border-zinc-600 text-zinc-400"}`}
                              title={i.confidence === "high" ? "Sure it is this app's" : "Named like the app — check it is this app's"}
                            >
                              {i.confidence === "high" ? "its own" : "maybe"}
                            </span>
                            {i.admin && (
                              <span className="shrink-0 text-zinc-400" title="Windows asks for permission to move this" data-leftover-elevate>
                                <Shield size={11} />
                              </span>
                            )}
                            {i.size != null && <span className="shrink-0 text-zinc-500">{formatBytes(i.size)}{i.size_complete ? "" : "+"}</span>}
                            {!NOT_FILES.includes(i.kind) && (
                              <button type="button" title="Show it" className="shrink-0 text-zinc-500 hover:text-zinc-300 cursor-pointer" onClick={() => revealItemInDir(i.path).catch(() => {})}>
                                <FolderOpen size={12} />
                              </button>
                            )}
                          </div>
                          <p className="text-zinc-500 break-words">{i.reason}</p>
                        </div>
                      </li>
                    ))}
                </ul>
              </section>
            ))}
            {adminItems.length > 0 && (
              <section className="rounded-lg border border-zinc-700 px-3 py-2 space-y-2" data-flow-admin>
                <h3 className="text-[11px] font-semibold uppercase tracking-wider text-zinc-300">Needs the administrator — GitSwitch doesn't delete these</h3>
                {adminItems.map((i) => (
                  <div key={i.id} className="space-y-1" data-leftover-admin={i.id}>
                    <div className="flex items-center gap-2 min-w-0">
                      <span className="font-mono text-[11px] text-zinc-300 truncate" title={i.path}>{i.path}</span>
                      <button type="button" title="Show it" className="shrink-0 text-zinc-500 hover:text-zinc-300 cursor-pointer" onClick={() => revealItemInDir(i.path).catch(() => {})}>
                        <FolderOpen size={12} />
                      </button>
                    </div>
                    <p className="text-zinc-500">{i.reason}</p>
                    {i.command && <CopyCommand command={i.command} />}
                  </div>
                ))}
              </section>
            )}
            <Lines items={scan.notes} className="text-zinc-500" />
            <div className="flex items-center gap-2 flex-wrap pt-1" data-flow-footer>
              {deletable.length === 0 ? (
                <Button size="sm" onClick={onClose} data-flow-close>
                  Done
                </Button>
              ) : ticked.size === 0 ? (
                <>
                  <span className="text-zinc-500" data-flow-none-ticked>
                    Nothing is ticked.
                  </span>
                  <Button variant="secondary" size="sm" onClick={onClose} data-flow-close>
                    Close
                  </Button>
                </>
              ) : (
                <>
                  <Button variant="danger" size="sm" onClick={deleteTicked} data-flow-delete>
                    <Trash2 size={14} /> Move {ticked.size} to the {trashWord}
                    {tickedSize > 0 ? ` (${formatBytes(tickedSize)})` : ""}
                  </Button>
                  <Button variant="ghost" size="sm" onClick={onClose}>
                    Keep them
                  </Button>
                  {tickedAdmin > 0 && (
                    <span className="flex items-center gap-1 text-zinc-400" data-flow-admin-note>
                      <Shield size={12} /> Windows asks for permission for {tickedAdmin === ticked.size ? (tickedAdmin === 1 ? "it" : "them") : `${tickedAdmin} of them`}.
                    </span>
                  )}
                </>
              )}
            </div>
          </div>
        )}

        {stage === "done" && (
          <div className="space-y-2" data-flow-done>
            <ul className="space-y-1">
              {results.map((r) => {
                const item = scan?.items.find((i) => i.id === r.id);
                const [label, cls] = r.outcome === "trashed" ? [`Moved to the ${trashWord}`, "text-emerald-400"] : OUTCOME[r.outcome] ?? [r.outcome, "text-zinc-300"];
                return (
                  <li key={r.id} data-result={r.outcome}>
                    <span className={cls}>{label}</span> <span className="font-mono text-[11px] text-zinc-400 break-all">{item?.path}</span>
                    {r.outcome !== "trashed" && r.outcome !== "removed" && <p className="text-zinc-500">{r.message}</p>}
                  </li>
                );
              })}
            </ul>
            <div className="flex items-center gap-2">
              {platform === "windows" && retryable.length > 0 && (
                <Button variant="secondary" size="sm" onClick={() => retryAdmin(retryable.map((r) => r.id))} data-flow-retry>
                  <Shield size={14} /> Try again
                </Button>
              )}
              <Button size="sm" onClick={onClose}>
                Done
              </Button>
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}
