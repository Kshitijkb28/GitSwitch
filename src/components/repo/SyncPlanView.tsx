import { AlertTriangle, XCircle } from "lucide-react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import type { SyncPlan, SyncSubPlan } from "../../lib/api";

const short = (oid: string | null | undefined) => (oid ? oid.slice(0, 7) : "—");

/** The plan's action, in the words a person would use. */
export function actionLabel(s: SyncSubPlan): string {
  switch (s.action) {
    case "none":
      return "nothing to do";
    case "fast-forward":
      return `fast-forward to ${short(s.target)}`;
    case "rebase":
      return `rebase your commits onto ${short(s.rebase_onto)}`;
    case "ahead":
      return "left alone (ahead)";
    case "blocked":
      return "blocked";
    default:
      return s.action;
  }
}

/** The button a remedy code gets, if any. Keyed on the code, never on the sentence. */
function remedyButton(code: string | null, sub: string | null): { label: string; code: string } | null {
  switch (code) {
    case "record-pointers":
      return { label: "Record submodule pointers", code };
    case "reinit":
      return { label: `Initialise ${sub ?? "it"}`, code };
    case "finish-operation":
    case "resolve-conflicts":
    case "stash-inside":
      return { label: `Open ${sub ?? "it"}`, code: "open" };
    case "turn-on-stash":
      return { label: "Turn on stashing", code };
    case "stash-collides":
      return { label: "Stash them now", code };
    case "untracked-in-the-way":
      return { label: "Go to Changes", code };
    default:
      return null;
  }
}

type Props = {
  plan: SyncPlan;
  stash: boolean;
  onStash: (v: boolean) => void;
  bundles: boolean;
  onBundles: (v: boolean) => void;
  busy: boolean;
  /** A remedy button was pressed: the code, and the submodule it is about. */
  onRemedy: (code: string, sub?: string) => void;
};

/**
 * What the update would do — the plan, read top to bottom: the sentence,
 * the blockers with one way out each, the branch's numbers, every submodule's
 * fate, and the two choices.
 */
export function SyncPlanView({ plan, stash, onStash, bundles, onBundles, busy, onRemedy }: Props) {
  const sup = plan.superproject;
  const superBlockers = plan.blockers.slice(0, sup.remedies?.length ?? 0);
  const blockedSubs = plan.submodules.filter((s) => s.action === "blocked");
  // Blockers the backend named but this view cannot attribute (an older
  // backend, or a sentence of its own): still shown, without a button.
  const otherBlockers = plan.blockers.slice(superBlockers.length).filter((b) => !blockedSubs.some((s) => b === `${s.path}: ${s.reason}`));

  return (
    <div className="space-y-3">
      <p className={`text-sm ${plan.refusal ? "text-red-200" : plan.nothing_to_do ? "text-zinc-300" : "text-zinc-100"}`}>{plan.summary}</p>

      {plan.refusal && (
        <div className="flex flex-wrap items-center gap-2 text-xs text-red-300">
          <XCircle size={13} className="shrink-0" />
          <span className="break-words">{plan.refusal.message}</span>
          {plan.refusal.code === "detached-head" && (
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => onRemedy("choose-branch")}>
              Choose a branch
            </Button>
          )}
        </div>
      )}

      {(superBlockers.length > 0 || blockedSubs.length > 0 || otherBlockers.length > 0) && (
        <ul className="space-y-1.5" aria-label="Blockers">
          {superBlockers.map((b, i) => {
            const btn = remedyButton(sup.remedies[i] ?? null, null);
            return (
              <li key={b} className="flex flex-wrap items-center gap-2 text-xs text-red-300">
                <XCircle size={13} className="shrink-0" />
                <span className="break-words">{b}</span>
                {btn && (
                  <Button size="sm" variant="secondary" disabled={busy} onClick={() => onRemedy(btn.code)}>
                    {btn.label}
                  </Button>
                )}
              </li>
            );
          })}
          {blockedSubs.map((s) => {
            const btn = remedyButton(s.remedy, s.path);
            return (
              <li key={s.path} className="flex flex-wrap items-center gap-2 text-xs text-red-300">
                <XCircle size={13} className="shrink-0" />
                <span className="break-words">
                  <span className="font-mono">{s.path}</span>: {s.reason}
                </span>
                {btn && (
                  <Button size="sm" variant="secondary" disabled={busy} onClick={() => onRemedy(btn.code, s.path)}>
                    {btn.label}
                  </Button>
                )}
              </li>
            );
          })}
          {otherBlockers.map((b) => (
            <li key={b} className="flex items-start gap-2 text-xs text-red-300">
              <XCircle size={13} className="shrink-0 mt-0.5" />
              <span className="break-words">{b}</span>
            </li>
          ))}
        </ul>
      )}

      {!plan.refusal && (
        <div className="text-xs text-zinc-400 space-y-1">
          <p>
            <span className="text-zinc-200">{sup.branch}</span> is {sup.ahead} ahead and {sup.behind} behind {sup.upstream}
            {sup.dirty_count > 0 && `, with ${sup.dirty_count} uncommitted change${sup.dirty_count === 1 ? "" : "s"}`}
            {sup.untracked > 0 && ` and ${sup.untracked} untracked file${sup.untracked === 1 ? "" : "s"}`}.
          </p>
          {sup.file_overlap.length > 0 && (
            <p className="text-amber-300/90 flex items-start gap-1.5">
              <AlertTriangle size={12} className="shrink-0 mt-0.5" />
              <span className="break-words">
                Both sides changed {sup.file_overlap.slice(0, 5).join(", ")}
                {sup.file_overlap.length > 5 && ` and ${sup.file_overlap.length - 5} more`} — the update pauses on any conflict so you can
                resolve it here.
              </span>
            </p>
          )}
          {sup.notes.map((n) => (
            <p key={n} className="break-words">
              {n}
            </p>
          ))}
          {sup.own_commits.length > 0 && (
            <details>
              <summary className="cursor-pointer select-none hover:text-zinc-300">
                Your {sup.own_commits.length} commit{sup.own_commits.length === 1 ? "" : "s"} that stay on top
              </summary>
              <ul className="mt-1 space-y-0.5 font-mono">
                {sup.own_commits.map((c) => (
                  <li key={c.oid} className="truncate">
                    {c.short} {c.subject}
                    {c.touches.length > 0 && <span className="text-zinc-500"> · moves {c.touches.join(", ")}</span>}
                  </li>
                ))}
              </ul>
            </details>
          )}
        </div>
      )}

      {plan.submodules.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-zinc-500 text-left">
                <th className="pr-3 pb-1 font-medium">Submodule</th>
                <th className="pr-3 pb-1 font-medium">On</th>
                <th className="pb-1 font-medium">What happens</th>
              </tr>
            </thead>
            <tbody>
              {plan.submodules.map((s) => (
                <tr key={s.path} className="align-top border-t border-zinc-800/60">
                  <td className="pr-3 py-1 font-mono text-zinc-200 whitespace-nowrap">{s.path}</td>
                  <td className="pr-3 py-1 text-zinc-400 whitespace-nowrap">{s.detached ? "detached" : s.branch ?? "—"}</td>
                  <td className="py-1">
                    <span className={s.action === "blocked" ? "text-red-300" : "text-zinc-200"}>{actionLabel(s)}</span>
                    <span className="text-zinc-500"> — {s.reason}</span>
                    {s.notes.map((n) => (
                      <p key={n} className="text-zinc-500">
                        {n}
                      </p>
                    ))}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {plan.skipped.length > 0 && (
        <details className="text-xs text-zinc-500">
          <summary className="cursor-pointer select-none hover:text-zinc-400">
            {plan.skipped.length} gitlink{plan.skipped.length === 1 ? "" : "s"} left alone
          </summary>
          <ul className="mt-1 space-y-0.5 font-mono">
            {plan.skipped.map((k) => (
              <li key={k.path} className="truncate">
                {k.path} <span className="font-sans">({k.why === "unmapped" ? "not in .gitmodules" : "not initialised"})</span>
              </li>
            ))}
          </ul>
        </details>
      )}

      {!plan.refusal && !plan.nothing_to_do && (
        <div className="space-y-1.5 pt-1">
          <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
            <Checkbox checked={stash} onChange={onStash} className="mt-0.5" aria-label="Stash uncommitted changes during the sync" />
            <span className="leading-relaxed">
              Stash my {sup.dirty_count} uncommitted change{sup.dirty_count === 1 ? "" : "s"} during the update
              <span className="text-zinc-500"> — and put them back afterwards</span>
            </span>
          </label>
          <label className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer">
            <Checkbox checked={bundles} onChange={onBundles} className="mt-0.5" aria-label="Also save bundle backups to a folder" />
            <span className="leading-relaxed">
              Also save bundle backups to a folder
              <span className="text-zinc-500">
                {" "}
                — a full copy of every branch in GitSwitch&apos;s data folder (sync-backups), outside the repository
              </span>
            </span>
          </label>
        </div>
      )}
    </div>
  );
}
