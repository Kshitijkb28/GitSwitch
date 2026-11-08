import { GitMerge, GitPullRequest, GitPullRequestClosed, GitPullRequestDraft } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { PullItem } from "../../lib/api";

export const cleanError = (e: unknown) => String(e).replace(/^(Error: )?(Command execution error: |Configuration error: )?/, "");

/** "3 hours ago" from an ISO time. */
export function ago(iso: string | null | undefined): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} minute${m === 1 ? "" : "s"} ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} hour${h === 1 ? "" : "s"} ago`;
  const d = Math.round(h / 24);
  if (d < 31) return `${d} day${d === 1 ? "" : "s"} ago`;
  return new Date(t).toLocaleDateString();
}

/** A small pill row: one choice of a few, like Repositories' All / Private / Public. */
export function Segmented<T extends string>({
  value,
  onChange,
  options,
  label,
  disabled,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
  label: string;
  disabled?: boolean;
}) {
  return (
    <div className="inline-flex rounded-lg bg-zinc-800/80 p-0.5 flex-wrap" role="group" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          aria-pressed={value === o.value}
          disabled={disabled}
          onClick={() => onChange(o.value)}
          className={`px-3 py-1 rounded-md text-xs font-medium transition-colors cursor-pointer disabled:cursor-not-allowed ${
            value === o.value ? "bg-zinc-700 text-emerald-400" : "text-zinc-400 hover:text-zinc-200"
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** The pull request's state as GitHub shows it: open, draft, merged or closed. */
export function PullStateIcon({ state, draft, size = 15 }: { state: PullItem["state"]; draft: boolean; size?: number }) {
  if (state === "merged") return <GitMerge size={size} className="text-violet-400 shrink-0" aria-label="Merged" />;
  if (state === "closed") return <GitPullRequestClosed size={size} className="text-red-400 shrink-0" aria-label="Closed" />;
  if (draft) return <GitPullRequestDraft size={size} className="text-zinc-400 shrink-0" aria-label="Draft" />;
  return <GitPullRequest size={size} className="text-emerald-400 shrink-0" aria-label="Open" />;
}

export function StateLabel({ state, draft }: { state: PullItem["state"]; draft: boolean }) {
  const [text, cls] =
    state === "merged"
      ? ["Merged", "bg-violet-500/15 text-violet-300 border-violet-500/30"]
      : state === "closed"
        ? ["Closed", "bg-red-500/15 text-red-300 border-red-500/30"]
        : draft
          ? ["Draft", "bg-zinc-700/50 text-zinc-300 border-zinc-600"]
          : ["Open", "bg-emerald-500/15 text-emerald-300 border-emerald-500/30"];
  return <span className={`inline-flex items-center px-2 py-0.5 rounded-full border text-[11px] font-medium ${cls}`}>{text}</span>;
}

export function openOnGitHub(url: string) {
  if (url) openUrl(url).catch(() => {});
}

export const STATE_OPTIONS: { value: "open" | "merged" | "closed" | "all"; label: string }[] = [
  { value: "open", label: "Open" },
  { value: "merged", label: "Merged" },
  { value: "closed", label: "Closed" },
  { value: "all", label: "All" },
];
