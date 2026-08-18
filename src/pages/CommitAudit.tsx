import { useState, useEffect, useRef } from "react";
import {
  History,
  Loader2,
  CheckCircle2,
  AlertTriangle,
  ShieldCheck,
  Shield,
  GitCommitHorizontal,
} from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Badge } from "../components/Badge";
import { Modal } from "../components/Modal";
import { useToast } from "../components/Toast";
import * as api from "../lib/api";

export function CommitAudit() {
  const toast = useToast();
  const [audits, setAudits] = useState<api.RepoAudit[]>([]);
  const [scanning, setScanning] = useState(false);
  const [hasRun, setHasRun] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmFix, setConfirmFix] = useState<api.RepoAudit | null>(null);
  const runSeq = useRef(0);

  async function scan() {
    const seq = ++runSeq.current;
    setScanning(true);
    setError(null);
    try {
      const res = await api.auditCommits();
      if (runSeq.current !== seq) return;
      setAudits(res);
      setHasRun(true);
    } catch (e) {
      if (runSeq.current !== seq) return;
      setError(String(e));
    } finally {
      if (runSeq.current === seq) setScanning(false);
    }
  }

  useEffect(() => {
    scan();
    return () => {
      runSeq.current++;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function doFix(a: api.RepoAudit) {
    setConfirmFix(null);
    setBusy(a.path);
    setError(null);
    try {
      const msg = await api.fixUnpushedCommits(a.path);
      toast.success(msg);
      await scan();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function toggleGuard(a: api.RepoAudit) {
    setBusy(a.path + ":guard");
    setError(null);
    try {
      const msg =
        a.guard === "gitswitch"
          ? await api.uninstallCommitGuard(a.path)
          : await api.installCommitGuard(a.path, a.expected_email);
      toast.success(msg);
      await scan();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const totalWrong = audits.reduce((n, a) => n + a.mismatched.length, 0);
  const totalFixable = audits.reduce((n, a) => n + a.unpushed_count, 0);

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div className="min-w-0">
          <h1 className="text-2xl font-bold text-zinc-100">Commit Audit</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Finds commits made with the wrong identity, and can rewrite the ones
            you haven't pushed yet.
          </p>
        </div>
        <Button onClick={scan} disabled={scanning}>
          {scanning ? (
            <>
              <Loader2 size={16} className="animate-spin" />
              Scanning…
            </>
          ) : (
            <>
              <History size={16} />
              Scan commits
            </>
          )}
        </Button>
      </div>

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      {scanning && audits.length === 0 && (
        <Card>
          <div className="flex items-center justify-center gap-2 py-8 text-sm text-zinc-400">
            <Loader2 size={16} className="animate-spin text-emerald-400" />
            Reading commit history across your profile folders…
          </div>
        </Card>
      )}

      {hasRun && !scanning && audits.length === 0 && (
        <Card className="text-center py-10">
          <CheckCircle2 size={36} className="mx-auto text-emerald-400 mb-3" />
          <h3 className="text-lg font-medium text-zinc-200">
            Every commit matches its folder
          </h3>
          <p className="text-sm text-zinc-500 mt-1">
            No commits were authored with the wrong identity.
          </p>
        </Card>
      )}

      {audits.length > 0 && (
        <div className="px-4 py-3 rounded-lg bg-amber-500/10 border border-amber-500/30 text-amber-300 text-sm">
          {totalWrong} commit{totalWrong === 1 ? "" : "s"} in {audits.length} repo
          {audits.length === 1 ? "" : "s"} carry the wrong identity —{" "}
          {totalFixable} not yet pushed and safely fixable.
        </div>
      )}

      {audits.map((a) => (
        <Card key={a.path}>
          <div className="flex items-start justify-between gap-3 mb-3 flex-wrap">
            <div className="min-w-0">
              <div className="flex items-center gap-2 flex-wrap">
                <h2 className="text-sm font-semibold text-zinc-100">{a.repo_name}</h2>
                <Badge variant="default">{a.profile_name}</Badge>
                {a.guard === "gitswitch" && <Badge variant="success">guarded</Badge>}
                {a.dirty && <Badge variant="warning">uncommitted changes</Badge>}
              </div>
              <p className="text-xs text-zinc-500 font-mono truncate mt-1">{a.path}</p>
              <p className="text-xs text-zinc-500 mt-1">
                should commit as{" "}
                <span className="text-emerald-400 font-mono">{a.expected_email}</span>
              </p>
            </div>
            <div className="flex items-center gap-2 shrink-0 ml-auto">
              <Button
                size="sm"
                variant="secondary"
                onClick={() => toggleGuard(a)}
                disabled={busy !== null}
                title={
                  a.guard === "foreign"
                    ? "This repo has its own pre-commit hook; GitSwitch will chain it"
                    : "Block future commits with the wrong identity"
                }
              >
                {busy === a.path + ":guard" ? (
                  <Loader2 size={14} className="animate-spin" />
                ) : a.guard === "gitswitch" ? (
                  <>
                    <ShieldCheck size={14} />
                    Guard on
                  </>
                ) : (
                  <>
                    <Shield size={14} />
                    Guard off
                  </>
                )}
              </Button>
              {a.unpushed_count > 0 && (
                <Button
                  size="sm"
                  onClick={() => setConfirmFix(a)}
                  disabled={busy !== null || a.dirty}
                  title={a.dirty ? "Commit or stash your changes first" : undefined}
                >
                  {busy === a.path ? (
                    <>
                      <Loader2 size={14} className="animate-spin" />
                      Rewriting…
                    </>
                  ) : (
                    `Fix ${a.unpushed_count} unpushed`
                  )}
                </Button>
              )}
            </div>
          </div>

          <div className="space-y-1">
            {a.mismatched.slice(0, 12).map((c) => (
              <div
                key={c.hash}
                className="flex items-center gap-2 sm:gap-3 px-3 py-2 rounded-lg bg-zinc-800/50 border border-zinc-700/40 text-xs min-w-0"
              >
                <GitCommitHorizontal size={14} className="text-zinc-600 shrink-0" />
                <span className="font-mono text-zinc-500 shrink-0">{c.short}</span>
                <span className="text-zinc-300 truncate flex-1 min-w-0">{c.subject}</span>
                <span
                  className="font-mono text-amber-400/90 truncate min-w-0 max-w-[40%] hidden md:inline"
                  title={c.author_email}
                >
                  {c.author_email}
                </span>
                <span className="shrink-0">
                  {c.pushed ? (
                    <Badge variant="default">pushed</Badge>
                  ) : (
                    <Badge variant="warning">local</Badge>
                  )}
                </span>
              </div>
            ))}
            {a.mismatched.length > 12 && (
              <p className="text-xs text-zinc-600 pl-3">
                …and {a.mismatched.length - 12} more
              </p>
            )}
          </div>

          {a.pushed_count > 0 && (
            <p className="text-xs text-zinc-600 mt-3">
              {a.pushed_count} of these {a.pushed_count === 1 ? "is" : "are"} already
              pushed. GitSwitch won't rewrite published history — that would need a
              force-push and would break anyone who has pulled it.
            </p>
          )}
        </Card>
      ))}

      <Modal
        open={confirmFix !== null}
        onClose={() => setConfirmFix(null)}
        title="Rewrite unpushed commits?"
      >
        <p className="text-sm text-zinc-400 mb-3">
          This rewrites the author of{" "}
          <strong className="text-zinc-200">
            {confirmFix?.unpushed_count} unpushed commit
            {confirmFix?.unpushed_count === 1 ? "" : "s"}
          </strong>{" "}
          in <span className="font-mono text-zinc-300">{confirmFix?.repo_name}</span> to{" "}
          <span className="font-mono text-emerald-400">{confirmFix?.expected_email}</span>.
        </p>
        <ul className="text-xs text-zinc-500 space-y-1.5 mb-4 list-disc pl-5">
          <li>Only commits that haven't been pushed are touched.</li>
          <li>Commit hashes change (that's unavoidable when rewriting authors).</li>
          <li>
            Your previous history is kept on a{" "}
            <span className="font-mono">gitswitch-backup-…</span> branch.
          </li>
        </ul>
        <div className="flex justify-end gap-3">
          <Button variant="secondary" onClick={() => setConfirmFix(null)}>
            Cancel
          </Button>
          <Button onClick={() => confirmFix && doFix(confirmFix)}>
            <AlertTriangle size={16} />
            Rewrite commits
          </Button>
        </div>
      </Modal>
    </div>
  );
}
