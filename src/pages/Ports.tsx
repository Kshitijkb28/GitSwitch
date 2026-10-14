import { useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Loader2, Square, Container as ContainerIcon, Zap } from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Badge } from "../components/Badge";
import { Modal } from "../components/Modal";
import { Input } from "../components/Input";
import { Checkbox } from "../components/Checkbox";
import { RefreshButton } from "../components/RefreshButton";
import { useToast } from "../components/Toast";
import { useRefreshOnFocus } from "../lib/focus";
import { usePersistedState } from "../lib/persist";
import { formatWhen } from "../lib/time";
import * as api from "../lib/api";

/** How often the list is read again while the page is on screen. */
const TICK_MS = 5000;

type Confirm =
  | { kind: "kill"; row: api.Listener; force: boolean }
  | { kind: "container"; row: api.Listener; project: boolean };

function machine(os: api.PortsReport["os"] | undefined): string {
  return os === "macos" ? "Mac" : os === "windows" ? "PC" : "machine";
}

function scopeWord(row: api.Listener): string {
  if (row.scope === "loopback") return "localhost";
  if (row.scope === "all") return "all interfaces";
  return row.addresses.join(", ");
}

function matches(row: api.Listener, needle: string): boolean {
  const n = needle.trim().toLowerCase();
  if (!n) return true;
  const hay = [
    String(row.port),
    row.name ?? "",
    row.cwd ?? "",
    row.clone?.name ?? "",
    row.clone?.path ?? "",
    row.clone?.profile_name ?? "",
    row.container?.name ?? "",
    row.container?.project ?? "",
    row.user ?? "",
  ];
  return hay.some((h) => h.toLowerCase().includes(n));
}

/**
 * Every listening port on this machine, the process that holds it and the
 * repository folder it runs from — and a way to stop it. Two copies of one
 * project collide through what one of them left running; this is where the
 * person sees which copy that was.
 */
export function Ports() {
  const toast = useToast();
  const [report, setReport] = useState<api.PortsReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = usePersistedState<string>("ports.filter", "");
  // null: not chosen yet — decided from the first listing (on when any row
  // belongs to one of the person's repositories, else off).
  const [onlyMine, setOnlyMine] = usePersistedState<boolean | null>("ports.onlyMine", null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  /** Rows whose Stop did not end them: Force stop is offered. */
  const [failedKeys, setFailedKeys] = useState<Set<string>>(new Set());
  const runSeq = useRef(0);
  const inFlight = useRef(false);

  // Opened from a repository's workspace line: "?clone=<folder>" becomes the
  // filter, once, and the address is left clean.
  const [params, setParams] = useSearchParams();
  useEffect(() => {
    const clone = params.get("clone");
    if (!clone) return;
    setFilter(clone);
    setParams({}, { replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function load() {
    // One listing at a time: a slow answer skips ticks instead of stacking them.
    if (inFlight.current) return;
    const seq = ++runSeq.current;
    inFlight.current = true;
    setLoading(true);
    try {
      const r = await api.portsList();
      if (runSeq.current !== seq) return;
      setReport(r);
      setError(null);
      if (onlyMine === null) setOnlyMine(r.rows.some((x) => x.clone !== null));
    } catch (e) {
      if (runSeq.current === seq) setError(String(e));
    } finally {
      inFlight.current = false;
      if (runSeq.current === seq) setLoading(false);
    }
  }

  useEffect(() => {
    void load();
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") void load();
    }, TICK_MS);
    return () => {
      window.clearInterval(id);
      runSeq.current++;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useRefreshOnFocus(() => void load());

  async function runKill(row: api.Listener, force: boolean) {
    if (row.pid === null) return;
    setBusyKey(row.key);
    setConfirm(null);
    try {
      const o = await api.portsKill(row.pid, force, row.name ?? "");
      if (o.ended) {
        toast.success(o.message);
        setFailedKeys((s) => {
          const n = new Set(s);
          n.delete(row.key);
          return n;
        });
      } else {
        toast.info(o.message);
        setFailedKeys((s) => new Set(s).add(row.key));
      }
    } catch (e) {
      toast.error(String(e).replace(/^Command execution error: /, ""));
    } finally {
      setBusyKey(null);
      void load();
    }
  }

  async function runContainerStop(row: api.Listener, project: boolean) {
    const c = row.container;
    if (!c) return;
    setBusyKey(row.key);
    setConfirm(null);
    try {
      const o = await api.portsContainerStop(c.id, project ? c.project : null);
      toast.success(o.message);
    } catch (e) {
      toast.error(String(e).replace(/^Command execution error: /, ""));
    } finally {
      setBusyKey(null);
      void load();
    }
  }

  const rows = report?.rows ?? [];
  const mine = onlyMine === true;
  const shown = rows.filter((r) => (!mine || r.clone !== null) && matches(r, filter));
  const mac = machine(report?.os);

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div className="min-w-0">
          <h1 className="text-2xl font-bold text-zinc-100">Ports</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Every listener on this {mac}, and which repository folder it runs from.
          </p>
        </div>
        <RefreshButton onClick={() => void load()} busy={loading} title="Read the ports again" />
      </div>

      {error && (
        <Card>
          <p className="text-sm text-red-300 break-words">{error}</p>
          <Button size="sm" variant="secondary" className="mt-2" onClick={() => void load()}>
            Retry
          </Button>
        </Card>
      )}

      <div className="flex items-center gap-3 flex-wrap">
        <Input
          aria-label="Filter ports"
          placeholder="Filter by port, process or folder"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          containerClassName="flex-1 min-w-[12rem]"
        />
        <label className="flex items-center gap-2 text-xs text-zinc-300 select-none cursor-pointer">
          <Checkbox aria-label="Only my repositories' folders" checked={mine} onChange={(v) => setOnlyMine(v)} />
          Only my repositories' folders
        </label>
        <span className="text-xs text-zinc-500">
          {shown.length} of {rows.length}
        </span>
      </div>

      {report && rows.length === 0 && (
        <Card>
          <p className="text-sm text-zinc-400">Nothing is listening on this {mac}.</p>
        </Card>
      )}
      {report && rows.length > 0 && shown.length === 0 && (
        <Card>
          {mine && !filter.trim() ? (
            <>
              <p className="text-sm text-zinc-400">Nothing is listening from your repositories' folders.</p>
              <Button size="sm" variant="ghost" className="mt-2" onClick={() => setOnlyMine(false)}>
                Show every port on this {mac} →
              </Button>
            </>
          ) : (
            <p className="text-sm text-zinc-400">No port, process or folder matches "{filter}".</p>
          )}
        </Card>
      )}

      {shown.length > 0 && (
        <Card className="p-0 overflow-hidden">
          <ul role="table" aria-label="Listening ports" className="divide-y divide-zinc-800/70">
            <li role="row" className="grid grid-cols-[7rem_minmax(0,1fr)_minmax(0,1.3fr)_auto] gap-x-3 px-3 py-2 text-xs font-medium text-zinc-500">
              <span>Port</span>
              <span>Process</span>
              <span className="hidden md:block">Where</span>
              <span className="text-right">Actions</span>
            </li>
            {shown.map((row) => {
              const busy = busyKey === row.key;
              const failed = failedKeys.has(row.key);
              const when = row.started_at ? formatWhen(row.started_at) : null;
              return (
                <li key={row.key} role="row" className="px-3 py-2 text-sm">
                  <div className="grid grid-cols-[7rem_minmax(0,1fr)_minmax(0,1.3fr)_auto] gap-x-3 items-start">
                    <div className="min-w-0">
                      <Badge>
                        {row.proto} {row.port}
                      </Badge>
                      <p className="text-[11px] text-zinc-500 mt-0.5 truncate" title={row.addresses.join(", ")}>
                        {scopeWord(row)}
                      </p>
                    </div>
                    <div className="min-w-0">
                      <p className="truncate text-zinc-200" title={row.exe ?? undefined}>
                        {row.name ?? "unknown"}
                        {row.pid !== null && <span className="text-zinc-500"> · pid {row.pid}</span>}
                      </p>
                      <p className="text-[11px] text-zinc-500 mt-0.5 flex items-center gap-1.5 flex-wrap">
                        {row.ours && <Badge variant="success">GitSwitch terminal</Badge>}
                        {row.user && !row.ours && row.pid !== null && !row.killable && row.why_not?.startsWith("Owned by") && (
                          <Badge variant="warning">{row.user}</Badge>
                        )}
                        {when && <span title={when.title}>started {when.text}</span>}
                      </p>
                    </div>
                    <div className="min-w-0 hidden md:block">
                      {row.clone ? (
                        <span className="inline-flex items-center gap-1.5 flex-wrap min-w-0">
                          <Badge variant="success">
                            {row.clone.name} · {row.clone.profile_name}
                          </Badge>
                          {row.copy && (
                            <span
                              className="text-[11px] text-zinc-500 whitespace-nowrap"
                              title={`The other ${row.copy.of === 2 ? "copy" : "copies"} of this project: ${row.copy.others.join(", ")}`}
                            >
                              copy {row.copy.rank} of {row.copy.of}
                            </span>
                          )}
                        </span>
                      ) : row.container ? (
                        <Badge>
                          {row.container.name}
                          {row.container.project ? ` · ${row.container.project}` : ""}
                        </Badge>
                      ) : row.cwd ? (
                        <p className="truncate text-xs text-zinc-400" title={row.cwd}>
                          {row.cwd}
                        </p>
                      ) : (
                        <p className="text-xs text-zinc-600">folder unknown</p>
                      )}
                      {row.clone && row.cwd && row.cwd !== row.clone.path && (
                        <p className="text-[11px] text-zinc-600 truncate mt-0.5" title={row.cwd}>
                          {row.cwd}
                        </p>
                      )}
                    </div>
                    <div className="flex flex-wrap justify-end items-center gap-1">
                      {row.container ? (
                        <>
                          <Button size="sm" variant="secondary" className="px-2" disabled={busy} onClick={() => setConfirm({ kind: "container", row, project: false })}>
                            {busy ? <Loader2 size={13} className="animate-spin" /> : <ContainerIcon size={13} />}
                            Stop container
                          </Button>
                          {row.container.project && (
                            <Button size="sm" variant="ghost" className="px-2" disabled={busy} onClick={() => setConfirm({ kind: "container", row, project: true })}>
                              Stop project {row.container.project}
                            </Button>
                          )}
                        </>
                      ) : row.killable ? (
                        <>
                          <Button size="sm" variant="danger" className="px-2" disabled={busy} onClick={() => setConfirm({ kind: "kill", row, force: false })}>
                            {busy ? <Loader2 size={13} className="animate-spin" /> : <Square size={13} />}
                            Stop
                          </Button>
                          {failed && (
                            <Button size="sm" variant="danger" className="px-2" disabled={busy} onClick={() => setConfirm({ kind: "kill", row, force: true })}>
                              <Zap size={13} />
                              Force stop
                            </Button>
                          )}
                        </>
                      ) : (
                        <Button size="sm" variant="ghost" className="px-2" disabled title={row.why_not ?? undefined}>
                          <Square size={13} />
                          Stop
                        </Button>
                      )}
                    </div>
                  </div>
                  {!row.killable && row.why_not && (
                    <p className="text-xs text-zinc-500 mt-1 break-words">{row.why_not}</p>
                  )}
                </li>
              );
            })}
          </ul>
        </Card>
      )}

      {report && report.notes.length > 0 && (
        <ul className="text-xs text-zinc-500 space-y-1">
          {report.notes.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      )}

      <Modal
        open={confirm?.kind === "kill"}
        onClose={() => setConfirm(null)}
        title={confirm?.kind === "kill" ? `${confirm.force ? "Force stop" : "Stop"} ${confirm.row.name ?? "this process"}?` : ""}
      >
        {confirm?.kind === "kill" && (
          <div className="space-y-3">
            <p className="text-sm text-zinc-300">
              Stop {confirm.row.name ?? "this process"} (pid {confirm.row.pid}) listening on {confirm.row.proto} {confirm.row.port}?
            </p>
            <p className="text-sm text-zinc-400">
              {confirm.row.clone
                ? `It was started from ${confirm.row.clone.name}.`
                : confirm.row.cwd
                  ? `It runs in ${confirm.row.cwd}.`
                  : "Its folder is unknown."}
            </p>
            {confirm.force && (
              <p className="text-sm text-amber-300/90">
                Force stop ends it immediately (SIGKILL on macOS and Linux, taskkill /F on Windows); whatever that process had not
                saved is lost.
              </p>
            )}
            <div className="flex justify-end gap-2">
              <Button size="sm" variant="secondary" onClick={() => setConfirm(null)}>
                Cancel
              </Button>
              <Button size="sm" variant="danger" onClick={() => void runKill(confirm.row, confirm.force)}>
                {confirm.force ? "Force stop" : "Stop"}
              </Button>
            </div>
          </div>
        )}
      </Modal>

      <Modal
        open={confirm?.kind === "container"}
        onClose={() => setConfirm(null)}
        title={confirm?.kind === "container" ? (confirm.project ? `Stop project ${confirm.row.container?.project}?` : `Stop container ${confirm.row.container?.name}?`) : ""}
      >
        {confirm?.kind === "container" && confirm.row.container && (
          <div className="space-y-3">
            <p className="text-sm text-zinc-300">
              {confirm.project
                ? `Stop every container of Compose project ${confirm.row.container.project}?`
                : `Stop container ${confirm.row.container.name}${confirm.row.container.project ? ` (project ${confirm.row.container.project})` : ""}? Its published port ${confirm.row.port} will close.`}
            </p>
            <div className="flex justify-end gap-2">
              <Button size="sm" variant="secondary" onClick={() => setConfirm(null)}>
                Cancel
              </Button>
              <Button size="sm" variant="danger" onClick={() => void runContainerStop(confirm.row, confirm.project)}>
                {confirm.project ? "Stop project" : "Stop container"}
              </Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
