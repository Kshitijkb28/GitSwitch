import { useEffect, useState } from "react";
import { Trash2, Loader2, ChevronDown, ChevronRight } from "lucide-react";
import { Modal } from "./Modal";
import { Button } from "./Button";
import { Checkbox } from "./Checkbox";
import * as api from "../lib/api";
import { useAnyJobRunning } from "../lib/gitJobs";

type Stage = "review" | "running" | "stopped" | "closing";

/** One line of a list, with an optional path under it. */
function Item({ text, path }: { text: string; path?: string | null }) {
  return (
    <li className="list-disc">
      <span className="text-zinc-300">{text}</span>
      {path && <span className="block font-mono text-[11px] text-zinc-500 break-all">{path}</span>}
    </li>
  );
}

/**
 * Uninstalling GitSwitch: what is taken back now, what goes once the app has
 * quit, what is kept — then the two steps (prepare, finish) the backend runs.
 */
export function UninstallDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const jobRunning = useAnyJobRunning();
  const [plan, setPlan] = useState<api.UninstallPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [keepData, setKeepData] = useState(false);
  const [showRepos, setShowRepos] = useState(false);
  const [stage, setStage] = useState<Stage>("review");
  const [step, setStep] = useState("");
  const [prepared, setPrepared] = useState<api.UninstallPrepare | null>(null);
  const [finished, setFinished] = useState<api.UninstallFinish | null>(null);

  useEffect(() => {
    if (!open) return;
    setPlan(null);
    setError(null);
    setStage("review");
    setPrepared(null);
    setFinished(null);
    api.uninstallPlan().then(setPlan, (e) => setError(String(e)));
  }, [open]);

  async function run() {
    if (!plan) return;
    setStage("running");
    setError(null);
    setStep(
      plan.lock_helper || plan.locks.length > 0
        ? "Removing the push locks — the administrator prompt will appear…"
        : "Taking back GitSwitch's changes…"
    );
    try {
      const p = await api.uninstallPrepare(keepData);
      setPrepared(p);
      if (p.outcome !== "done") {
        setStage("stopped");
        return;
      }
      setStep("Closing GitSwitch…");
      setFinished(await api.uninstallFinish(keepData));
      setStage("closing");
    } catch (e) {
      setError(String(e));
      setStage("stopped");
    }
  }

  const busy = stage === "running" || stage === "closing";
  const locks = plan ? plan.locks.length : 0;

  return (
    <Modal open={open} onClose={busy ? () => {} : onClose} title="Uninstall GitSwitch" size="lg">
      <div className="max-h-[70vh] overflow-y-auto pr-1 space-y-4 text-xs text-zinc-400" data-uninstall-dialog>
        {error && (
          <p className="whitespace-pre-wrap break-all rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-red-400" data-uninstall-error>
            {error}
          </p>
        )}
        {!plan && !error && (
          <p className="flex items-center gap-2">
            <Loader2 size={14} className="animate-spin" /> Reading what GitSwitch has set up on this computer…
          </p>
        )}
        {plan && stage === "review" && (
          <>
            <section data-uninstall-now>
              <h3 className="text-[11px] font-semibold uppercase tracking-wider text-zinc-300 mb-1">Taken back now</h3>
              <ul className="pl-4 space-y-1">
                {(plan.lock_helper || locks > 0) && (
                  <Item text={`The push locks (${locks}) and the lock helper — the administrator prompt will appear`} />
                )}
                {plan.repos.length > 0 && (
                  <li className="list-disc">
                    <button
                      type="button"
                      className="inline-flex items-center gap-1 text-zinc-300 hover:text-zinc-100 cursor-pointer"
                      onClick={() => setShowRepos((v) => !v)}
                      data-uninstall-repos={plan.repos.length}
                    >
                      Push blocks, identity guards and GitSwitch's settings in {plan.repos.length} repositor
                      {plan.repos.length === 1 ? "y" : "ies"}
                      {showRepos ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                    </button>
                    {showRepos && (
                      <ul className="mt-1 space-y-0.5">
                        {plan.repos.map((r) => (
                          <li key={r} className="font-mono text-[11px] text-zinc-500 break-all">{r}</li>
                        ))}
                      </ul>
                    )}
                  </li>
                )}
                {plan.gitconfig && <Item text="GitSwitch's block in your git config (a backup is kept beside it)" path={plan.gitconfig} />}
                {plan.signers && <Item text="GitSwitch's entries in allowed_signers (your own lines stay)" path={plan.signers} />}
                {plan.startup && <Item text="Starting GitSwitch when you sign in (the Net speed tab's choice)" />}
                {!keepData && plan.signins.length > 0 && (
                  <Item text={`GitHub sign-ins kept by GitSwitch: ${plan.signins.map((s) => "@" + s).join(", ")}`} />
                )}
                {!plan.lock_helper && locks === 0 && plan.repos.length === 0 && !plan.gitconfig && !plan.signers && !plan.startup && (keepData || plan.signins.length === 0) && (
                  <li className="list-none text-zinc-500">Nothing — GitSwitch changed nothing outside its own folders.</li>
                )}
              </ul>
            </section>
            <section data-uninstall-later>
              <h3 className="text-[11px] font-semibold uppercase tracking-wider text-zinc-300 mb-1">Removed once GitSwitch has quit</h3>
              <ul className="pl-4 space-y-1">
                {!keepData && plan.data.map((d) => <Item key={d} text="GitSwitch's data (profiles, logs, update backups, window data)" path={d} />)}
                <Item text={plan.how} />
              </ul>
            </section>
            <section data-uninstall-kept>
              <h3 className="text-[11px] font-semibold uppercase tracking-wider text-zinc-300 mb-1">Kept</h3>
              <ul className="pl-4 space-y-1">
                <Item text="Your SSH keys (they may be registered on GitHub)" />
                <Item text="Your repositories and their files" />
                <Item text="The GitHub CLI's logins (gh)" />
                {keepData && <Item text="Your profiles, settings and GitHub sign-ins — reinstall GitSwitch and press Re-apply Config in Settings" />}
              </ul>
            </section>
            <label className="flex items-center gap-2 cursor-pointer text-zinc-300">
              <Checkbox checked={keepData} onChange={setKeepData} aria-label="Keep my profiles and sign-ins" />
              Keep my profiles and sign-ins for a later reinstall
            </label>
            <div className="flex items-center gap-2 flex-wrap pt-1">
              <Button variant="danger" size="sm" onClick={run} disabled={jobRunning} data-uninstall-go>
                <Trash2 size={14} />
                Uninstall GitSwitch
              </Button>
              <Button variant="ghost" size="sm" onClick={onClose}>
                Cancel
              </Button>
              {jobRunning && <span className="text-amber-300/90">Wait for the running git operation to finish first.</span>}
            </div>
          </>
        )}
        {busy && (
          <p className="flex items-center gap-2 text-zinc-300" data-uninstall-step>
            <Loader2 size={14} className="animate-spin" /> {step}
          </p>
        )}
        {prepared && (prepared.done.length > 0 || prepared.problems.length > 0) && (
          <ul className="pl-4 space-y-1" data-uninstall-done>
            {prepared.done.map((d) => <li key={d} className="list-disc text-emerald-400/90">{d}</li>)}
            {prepared.problems.map((d) => <li key={d} className="list-disc text-amber-300/90 break-all">{d}</li>)}
          </ul>
        )}
        {stage === "stopped" && prepared && (
          <div className="space-y-2" data-uninstall-stopped>
            <p className="text-amber-300/90 whitespace-pre-wrap">{prepared.message}</p>
            {prepared.command && (
              <pre className="whitespace-pre-wrap break-all rounded-lg border border-zinc-700 bg-zinc-900 px-3 py-2 font-mono text-zinc-300">{prepared.command}</pre>
            )}
            <Button variant="ghost" size="sm" onClick={onClose}>Close</Button>
          </div>
        )}
        {stage === "closing" && finished && (
          <div className="space-y-1" data-uninstall-closing>
            <p className="text-zinc-300">{finished.how}</p>
            {finished.manual && <p className="text-amber-300/90" data-uninstall-manual>{finished.manual}</p>}
            <p className="text-zinc-500">What the removal did is written to <span className="font-mono break-all">{finished.log}</span>.</p>
          </div>
        )}
      </div>
    </Modal>
  );
}
