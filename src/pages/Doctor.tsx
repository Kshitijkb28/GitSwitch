import { useState, useEffect, useRef } from "react";
import {
  Stethoscope,
  Loader2,
  CheckCircle2,
  XCircle,
  AlertTriangle,
  Info,
  Wrench,
} from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { useToast } from "../components/Toast";
import * as api from "../lib/api";
import { notePushChanged } from "../lib/console";
import { copyText } from "../lib/clipboard";
import { useNavigate } from "react-router-dom";
import { RefreshButton } from "../components/RefreshButton";

type Step = {
  key: string;
  label: string;
  run: () => Promise<api.Finding[]>;
};

const STEPS: Step[] = [
  { key: "env", label: "Environment & git config", run: api.doctorCheckEnvironment },
  { key: "profiles", label: "Profiles & folders", run: api.doctorCheckProfiles },
  { key: "repos", label: "Repository remotes", run: api.doctorCheckRepos },
  { key: "keys", label: "SSH key ↔ GitHub account", run: api.doctorCheckKeys },
  { key: "locks", label: "Push locks", run: api.doctorCheckPushLocks },
];

const SEVERITY_ORDER: Record<string, number> = { error: 0, warning: 1, info: 2, ok: 3 };

export function Doctor() {
  const toast = useToast();
  const navigate = useNavigate();
  const [findings, setFindings] = useState<api.Finding[]>([]);
  const [running, setRunning] = useState(false);
  const [currentStep, setCurrentStep] = useState<string | null>(null);
  const [doneSteps, setDoneSteps] = useState<string[]>([]);
  const [hasRun, setHasRun] = useState(false);
  const [fixing, setFixing] = useState<string | null>(null);
  /** The access check, which asks GitHub about every repository: on request only. */
  const [checkingAccess, setCheckingAccess] = useState(false);
  const [accessChecked, setAccessChecked] = useState(false);
  /** Its findings, kept apart: a re-run of the other checks does not drop them. */
  const [accessFindings, setAccessFindings] = useState<api.Finding[]>([]);
  const [error, setError] = useState<string | null>(null);
  const runSeq = useRef(0);

  async function runChecks() {
    const seq = ++runSeq.current;
    setRunning(true);
    setError(null);
    setFindings([]);
    setDoneSteps([]);
    setHasRun(true);

    const collected: api.Finding[] = [];
    for (const step of STEPS) {
      if (runSeq.current !== seq) return; // superseded by a newer run
      setCurrentStep(step.key);
      try {
        const res = await step.run();
        if (runSeq.current !== seq) return;
        collected.push(...res);
        // Show results as each step lands rather than waiting for all of them.
        setFindings([...collected]);
      } catch (e) {
        if (runSeq.current !== seq) return;
        collected.push({
          id: `${step.key}-failed`,
          severity: "info",
          title: `Couldn't run "${step.label}"`,
          detail: String(e),
          fix: null,
          fix_label: null,
        });
        setFindings([...collected]);
      }
      setDoneSteps((prev) => [...prev, step.key]);
    }

    if (runSeq.current === seq) {
      setCurrentStep(null);
      setRunning(false);
    }
  }

  // Run once when the page opens — the whole point is a zero-effort check-up.
  useEffect(() => {
    runChecks();
    return () => {
      runSeq.current++; // invalidate any in-flight run on unmount
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function checkAccess() {
    setCheckingAccess(true);
    setError(null);
    try {
      const res = await api.doctorCheckRepoAccess();
      setAccessFindings(res);
      setAccessChecked(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setCheckingAccess(false);
    }
  }

  async function applyFix(finding: api.Finding) {
    if (!finding.fix) return;
    setFixing(finding.id + finding.title);
    setError(null);
    try {
      if (finding.fix.startsWith("nav:")) {
        navigate(finding.fix.slice("nav:".length));
        return;
      } else if (finding.fix.startsWith("copy:")) {
        const ok = await copyText(finding.fix.slice("copy:".length));
        if (ok) toast.success("Copied — paste it into a terminal");
        else setError(`Couldn't copy — run it by hand:\n${finding.fix.slice("copy:".length)}`);
        return;
      } else if (/^(drop-folder|register-key|register-signing|assign):/.test(finding.fix)) {
        toast.success(await api.doctorApplyFix(finding.fix));
      } else if (finding.fix === "fix-ssh-config") {
        const msg = await api.doctorFixSshConfig();
        toast.success(msg);
      } else if (finding.fix.startsWith("convert-ssh:")) {
        const path = finding.fix.slice("convert-ssh:".length);
        const changes = await api.convertReposToSsh(path);
        const changed = changes.filter((c) => c.changed);
        toast.success(changed.length > 0 ? changed[0].note || "Remote converted to SSH" : "Nothing needed converting");
      } else if (finding.fix.startsWith("unset-local:")) {
        // unset-local:<key>:<repo path> — the key has no colon; the path may.
        const rest = finding.fix.slice("unset-local:".length);
        const cut = rest.indexOf(":");
        const msg = await api.doctorUnsetLocal(rest.slice(cut + 1), rest.slice(0, cut));
        toast.success(msg);
      } else if (finding.fix.startsWith("respell-folders:")) {
        // Saving the profile again stores each folder as the disk spells it.
        const id = finding.fix.slice("respell-folders:".length);
        const profile = (await api.getProfiles()).find((p) => p.id === id);
        if (profile) {
          await api.updateProfile({ id, directories: profile.directories });
          toast.success(`"${profile.name}" now names its folders as the disk spells them`);
        }
      } else if (finding.fix.startsWith("lock-")) {
        // Most of these open the administrator prompt; the outcome — cancelled
        // included — comes back as a sentence rather than an exception.
        const r = await api.pushLockFix(finding.fix);
        if (r.outcome === "applied") {
          notePushChanged();
          toast.success(r.message);
        } else if (r.outcome === "manual-required" && r.command) {
          setError(`${r.message}\n${r.command}`);
        } else {
          setError(r.message);
        }
      }
      await runChecks();
      // A repository moved to another profile: what GitHub says about access
      // is asked again, so the finding that was fixed goes and the rest stay.
      if (finding.fix.startsWith("assign:") && accessChecked) await checkAccess();
    } catch (e) {
      setError(String(e));
    } finally {
      setFixing(null);
    }
  }

  const all = [...findings, ...accessFindings];
  const sorted = [...all].sort(
    (a, b) => (SEVERITY_ORDER[a.severity] ?? 9) - (SEVERITY_ORDER[b.severity] ?? 9)
  );
  const counts = {
    error: all.filter((f) => f.severity === "error").length,
    warning: all.filter((f) => f.severity === "warning").length,
    info: all.filter((f) => f.severity === "info").length,
  };
  const allClear = hasRun && !running && !checkingAccess && all.length === 0;

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Doctor</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Checks the things that silently send commits to the wrong account —
            ssh config overrides, unregistered keys, HTTPS remotes and leaked tokens —
            and that every push lock is still in place.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <RefreshButton onClick={runChecks} busy={running} title="Run the checks again" />
          <Button onClick={runChecks} disabled={running}>
            {running ? (
              <>
                <Loader2 size={16} className="animate-spin" />
                Checking…
              </>
            ) : (
              <>
                <Stethoscope size={16} />
                Run checks
              </>
            )}
          </Button>
        </div>
      </div>

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words whitespace-pre-wrap">
          {error}
        </div>
      )}

      <Card>
        <div className="space-y-2">
          <div className="flex items-center gap-2.5 text-sm flex-wrap">
            {accessChecked && !checkingAccess ? (
              <CheckCircle2 size={15} className="text-emerald-400 shrink-0" />
            ) : checkingAccess ? (
              <Loader2 size={15} className="animate-spin text-emerald-400 shrink-0" />
            ) : (
              <span className="w-[15px] h-[15px] rounded-full border border-zinc-700 shrink-0" />
            )}
            <span className={accessChecked || checkingAccess ? "text-zinc-300" : "text-zinc-600"}>
              Repository access on GitHub
            </span>
            {!checkingAccess && (
              <button
                type="button"
                onClick={checkAccess}
                disabled={running}
                className="text-xs text-emerald-400 hover:text-emerald-300 cursor-pointer disabled:text-zinc-600"
                title="Asks GitHub, with your gh logins, whether the account each repository pushes as can see it"
              >
                {accessChecked ? "Check again" : "Check now"}
              </button>
            )}
          </div>
          {STEPS.map((s) => {
            const done = doneSteps.includes(s.key);
            const active = currentStep === s.key;
            return (
              <div key={s.key} className="flex items-center gap-2.5 text-sm">
                {done ? (
                  <CheckCircle2 size={15} className="text-emerald-400 shrink-0" />
                ) : active ? (
                  <Loader2 size={15} className="animate-spin text-emerald-400 shrink-0" />
                ) : (
                  <span className="w-[15px] h-[15px] rounded-full border border-zinc-700 shrink-0" />
                )}
                <span className={done || active ? "text-zinc-300" : "text-zinc-600"}>
                  {s.label}
                </span>
              </div>
            );
          })}
        </div>

        {hasRun && !running && (
          <div className="mt-4 pt-4 border-t border-zinc-800 flex items-center gap-4 text-sm">
            <span className="inline-flex items-center gap-1.5 text-red-400">
              <XCircle size={15} /> {counts.error} problem{counts.error === 1 ? "" : "s"}
            </span>
            <span className="inline-flex items-center gap-1.5 text-amber-400">
              <AlertTriangle size={15} /> {counts.warning} warning{counts.warning === 1 ? "" : "s"}
            </span>
            <span className="inline-flex items-center gap-1.5 text-zinc-500">
              <Info size={15} /> {counts.info} note{counts.info === 1 ? "" : "s"}
            </span>
          </div>
        )}
      </Card>

      {allClear && (
        <Card className="text-center py-10">
          <CheckCircle2 size={36} className="mx-auto text-emerald-400 mb-3" />
          <h3 className="text-lg font-medium text-zinc-200">Everything checks out</h3>
          <p className="text-sm text-zinc-500 mt-1">
            No config conflicts, every key is registered, and no repo bypasses your profiles.
          </p>
        </Card>
      )}

      {sorted.map((f, i) => (
        <FindingCard
          key={`${f.id}-${i}`}
          finding={f}
          fixing={fixing === f.id + f.title}
          onFix={() => applyFix(f)}
        />
      ))}
    </div>
  );
}

function FindingCard({
  finding,
  fixing,
  onFix,
}: {
  finding: api.Finding;
  fixing: boolean;
  onFix: () => void;
}) {
  const style =
    finding.severity === "error"
      ? { icon: <XCircle size={18} className="text-red-400 shrink-0 mt-0.5" />, border: "border-red-500/30" }
      : finding.severity === "warning"
      ? { icon: <AlertTriangle size={18} className="text-amber-400 shrink-0 mt-0.5" />, border: "border-amber-500/30" }
      : { icon: <Info size={18} className="text-zinc-500 shrink-0 mt-0.5" />, border: "border-zinc-700/50" };

  return (
    <div className={`rounded-xl border ${style.border} bg-zinc-900/60 p-4`}>
      <div className="flex items-start gap-3 min-w-0">
        {style.icon}
        <div className="min-w-0 flex-1">
          <h3 className="text-sm font-semibold text-zinc-100 break-words">{finding.title}</h3>
          <p className="text-xs text-zinc-400 mt-1 whitespace-pre-wrap break-words">
            {finding.detail}
          </p>
        </div>
        {finding.fix && (
          <Button size="sm" variant="secondary" onClick={onFix} disabled={fixing}>
            {fixing ? (
              <>
                <Loader2 size={14} className="animate-spin" />
                Fixing…
              </>
            ) : (
              <>
                <Wrench size={14} />
                {finding.fix_label ?? "Fix"}
              </>
            )}
          </Button>
        )}
      </div>
    </div>
  );
}
