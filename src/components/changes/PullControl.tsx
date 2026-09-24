import { GitMerge, FastForward, Layers } from "lucide-react";
import { Checkbox } from "../Checkbox";
import type { PullMode } from "../../lib/api";

interface Props {
  mode: PullMode;
  onMode: (m: PullMode) => void;
  ahead: number;
  behind: number;
  /** Any staged or unstaged change in the worktree. */
  dirty: boolean;
  upstream: string | null;
  /** Rebase only: stash the dirty tree around the rebase and put it back after. */
  autostash?: boolean;
  onAutostash?: (v: boolean) => void;
}

type Option = {
  value: PullMode;
  label: string;
  icon: typeof GitMerge;
  what: string;
};

const OPTIONS: Option[] = [
  {
    value: "ff-only",
    label: "Fast-forward",
    icon: FastForward,
    what: "Only moves your branch forward. Refuses if that isn't possible.",
  },
  {
    value: "merge",
    label: "Merge",
    icon: GitMerge,
    what: "Joins the two histories with a merge commit. Your commits keep their hashes.",
  },
  {
    value: "rebase",
    label: "Rebase",
    icon: Layers,
    what: "Replays your commits on top of theirs. Your commits get new hashes.",
  },
];

/**
 * Why the mode is a visible choice and not a setting: `git pull` behaves
 * differently depending on pull.rebase / pull.ff, so the same button would
 * merge for one person and rebase for another. Here the word on the button is
 * the command that runs.
 */
export function whyDisabled(
  mode: PullMode,
  ahead: number,
  behind: number,
  dirty: boolean,
  autostash = false
): string | null {
  if (mode === "ff-only" && ahead > 0 && behind > 0) {
    return `Not possible: you have ${ahead} commit${ahead === 1 ? "" : "s"} the remote doesn't have, so the branch can't just move forward.`;
  }
  if (mode === "rebase" && dirty && !autostash) {
    return "Rebase needs a clean working tree. Commit or stash your changes first — or turn on autostash below.";
  }
  return null;
}

/** The sentence shown before anything runs, computed from ahead/behind. */
export function consequence(
  mode: PullMode,
  ahead: number,
  behind: number,
  upstream: string | null
): string {
  const up = upstream ?? "the remote";
  const theirs = `${behind} new commit${behind === 1 ? "" : "s"} from ${up}`;
  const mine = `${ahead} local commit${ahead === 1 ? "" : "s"}`;

  if (behind === 0) {
    return ahead > 0
      ? `Nothing to pull — ${up} has no commits you don't already have. Your ${mine} are still unpushed.`
      : `Nothing to pull — you're level with ${up}.`;
  }
  if (ahead === 0) {
    return `Your branch moves forward to ${up}, picking up ${theirs}. Nothing of yours changes.`;
  }
  switch (mode) {
    case "ff-only":
      return `Can't fast-forward: you have ${mine} and ${up} has ${behind} you don't.`;
    case "merge":
      return `Creates a merge commit joining your ${mine} with ${theirs}. Your commit hashes stay the same.`;
    case "rebase":
      return `Your ${mine} are replayed on top of ${theirs}. Those ${ahead} commit${ahead === 1 ? "" : "s"} get new hashes.`;
  }
}

export function PullControl({
  mode,
  onMode,
  ahead,
  behind,
  dirty,
  upstream,
  autostash = false,
  onAutostash,
}: Props) {
  const stashing = mode === "rebase" && autostash;
  const blockedNow = whyDisabled(mode, ahead, behind, dirty, stashing);
  return (
    <div className="space-y-2 min-w-0">
      <div className="flex rounded-lg border border-zinc-700 overflow-hidden w-fit max-w-full">
        {OPTIONS.map((o) => {
          const Icon = o.icon;
          const blocked = whyDisabled(o.value, ahead, behind, dirty, o.value === "rebase" && autostash);
          const active = mode === o.value;
          return (
            <button
              key={o.value}
              onClick={() => onMode(o.value)}
              title={blocked ?? o.what}
              className={`flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer border-r border-zinc-700 last:border-r-0 ${
                active
                  ? "bg-emerald-600 text-white"
                  : blocked
                    ? "bg-zinc-800 text-zinc-600 hover:bg-zinc-800"
                    : "bg-zinc-800 text-zinc-300 hover:bg-zinc-700"
              }`}
            >
              <Icon size={13} className="shrink-0" />
              <span>{o.label}</span>
            </button>
          );
        })}
      </div>
      <p className="text-xs text-zinc-400 leading-relaxed">
        {consequence(mode, ahead, behind, upstream)}
      </p>
      {blockedNow && (
        <p className="text-xs text-amber-400/90 leading-relaxed">{blockedNow}</p>
      )}
      {mode === "rebase" && onAutostash && (
        <label
          className="flex items-start gap-2 text-xs text-zinc-300 cursor-pointer"
          title="Runs git pull --rebase --autostash: your uncommitted changes are stashed before the rebase and re-applied after it"
        >
          <Checkbox
            checked={autostash}
            onChange={onAutostash}
            className="mt-0.5"
            aria-label="Stash changes around the rebase"
          />
          <span className="leading-relaxed">
            Stash my changes around the rebase (autostash)
            <span className="text-zinc-500">
              {" "}
              — if they don't re-apply cleanly, they stay in the stash and the conflicts show here
            </span>
          </span>
        </label>
      )}
    </div>
  );
}
