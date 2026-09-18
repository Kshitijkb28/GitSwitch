import { GitMerge, FastForward, Layers } from "lucide-react";
import type { PullMode } from "../../lib/api";

interface Props {
  mode: PullMode;
  onMode: (m: PullMode) => void;
  ahead: number;
  behind: number;
  /** Any staged or unstaged change in the worktree. */
  dirty: boolean;
  upstream: string | null;
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
  dirty: boolean
): string | null {
  if (mode === "ff-only" && ahead > 0 && behind > 0) {
    return `Not possible: you have ${ahead} commit${ahead === 1 ? "" : "s"} the remote doesn't have, so the branch can't just move forward.`;
  }
  if (mode === "rebase" && dirty) {
    return "Rebase needs a clean working tree. Commit or stash your changes first.";
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

export function PullControl({ mode, onMode, ahead, behind, dirty, upstream }: Props) {
  return (
    <div className="space-y-2 min-w-0">
      <div className="flex rounded-lg border border-zinc-700 overflow-hidden w-fit max-w-full">
        {OPTIONS.map((o) => {
          const Icon = o.icon;
          const blocked = whyDisabled(o.value, ahead, behind, dirty);
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
      {whyDisabled(mode, ahead, behind, dirty) && (
        <p className="text-xs text-amber-400/90 leading-relaxed">
          {whyDisabled(mode, ahead, behind, dirty)}
        </p>
      )}
    </div>
  );
}
