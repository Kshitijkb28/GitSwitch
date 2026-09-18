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
];

const SEVERITY_ORDER: Record<string, number> = { error: 0, warning: 1, info: 2, ok: 3 };

export function Doctor() {
  const toast = useToast();
  const [findings, setFindings] = useState<api.Finding[]>([]);
  const [running, setRunning] = useState(false);
  const [currentStep, setCurrentStep] = useState<string | null>(null);
  const [doneSteps, setDoneSteps] = useState<string[]>([]);
  const [hasRun, setHasRun] = useState(false);
  const [fixing, setFixing] = useState<string | null>(null);
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

  async function applyFix(finding: api.Finding) {
    if (!finding.fix) return;
    setFixing(finding.id + finding.title);
    setError(null);
    try {
      if (finding.fix === "fix-ssh-config") {
        const msg = await api.doctorFixSshConfig();
        toast.success(msg);
      } else if (finding.fix.startsWith("convert-ssh:")) {
        const path = finding.fix.slice("convert-ssh:".length);
        const changes = await api.convertReposToSsh(path);
        const changed = changes.filter((c) => c.changed).length;
        toast.success(
          changed > 0 ? "Remote converted to SSH" : "Nothing needed converting"
        );
      }
      await runChecks();
    } catch (e) {
      setError(String(e));
    } finally {
      setFixing(null);
    }
  }

  const sorted = [...findings].sort(
    (a, b) => (SEVERITY_ORDER[a.severity] ?? 9) - (SEVERITY_ORDER[b.severity] ?? 9)
  );
  const counts = {
    error: findings.filter((f) => f.severity === "error").length,
    warning: findings.filter((f) => f.severity === "warning").length,
    info: findings.filter((f) => f.severity === "info").length,
  };
  const allClear = hasRun && !running && findings.length === 0;

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Doctor</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Checks the things that silently send commits to the wrong account —
            ssh config overrides, unregistered keys, HTTPS remotes and leaked tokens.
          </p>
        </div>
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

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
          {error}
        </div>
      )}

      <Card>
        <div className="space-y-2">
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
