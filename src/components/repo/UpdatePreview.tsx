import { formatWhen } from "../../lib/time";
import type { SyncCommitRef, SyncPlan } from "../../lib/api";

const TONES = {
  you: "bg-emerald-400",
  in: "bg-sky-400",
  dropped: "border border-amber-400 bg-transparent",
  bump: "bg-zinc-600",
} as const;

function Dot({ tone }: { tone: keyof typeof TONES }) {
  return <span className={`inline-block w-2.5 h-2.5 rounded-full shrink-0 ${TONES[tone]}`} aria-hidden="true" />;
}

/** One rail: newest at the top, the shared base at the bottom. */
function Rail({
  title,
  tone,
  commits,
  more,
  moreNote,
  base,
}: {
  title: string;
  tone: "you" | "in";
  commits: SyncCommitRef[];
  more: number;
  /** Something true of the commits past the listed ones. */
  moreNote?: string;
  base: string;
}) {
  return (
    <div className="min-w-0">
      <p className={`font-medium mb-1 ${tone === "you" ? "text-emerald-300" : "text-sky-300"}`}>{title}</p>
      <ul className="border-l border-zinc-700 ml-1 pl-3 space-y-1">
        {commits.length === 0 && <li className="text-zinc-600">nothing new</li>}
        {commits.map((c) => {
          const when = c.date ? formatWhen(c.date) : null;
          const t = c.already_upstream ? "dropped" : c.only_pointers ? "bump" : tone;
          return (
            <li key={c.oid} className="flex flex-wrap items-baseline gap-x-2 min-w-0">
              <Dot tone={t} />
              <span className="font-mono text-zinc-500 shrink-0">{c.short}</span>
              <span className={`truncate min-w-0 ${c.already_upstream ? "text-zinc-500 line-through" : "text-zinc-300"}`}>{c.subject}</span>
              {c.touches.length > 0 && <span className="text-zinc-500 shrink-0">moves {c.touches.join(", ")}</span>}
              {c.only_pointers && <span className="text-[10px] px-1 rounded bg-zinc-800 text-zinc-500 shrink-0">pointer bump</span>}
              {c.already_upstream && <span className="text-[10px] px-1 rounded bg-amber-500/15 text-amber-300 shrink-0">already there — dropped</span>}
              {(c.author_name || when) && (
                <span className="text-zinc-600 shrink-0" title={when?.title}>
                  {c.author_name}
                  {c.author_name && when ? " · " : ""}
                  {when?.text}
                </span>
              )}
            </li>
          );
        })}
        {more > 0 && (
          <li className="text-zinc-600">
            +{more} more{moreNote ? ` (${moreNote})` : ""}
          </li>
        )}
        <li className="flex items-baseline gap-x-2 text-zinc-500">
          <span className="inline-block w-2.5 h-2.5 rounded-full bg-zinc-500 shrink-0" aria-hidden="true" />
          <span>
            where you and the remote last agreed{base ? ` · ${base}` : ""}
          </span>
        </li>
      </ul>
    </div>
  );
}

/**
 * The picture before it runs: your commits and theirs from the last point
 * you agreed on, and what the branch looks like afterwards. Read it, then
 * press Update now — or close the drawer.
 */
export function UpdatePreview({ plan }: { plan: SyncPlan }) {
  const sup = plan.superproject;
  // Counted from the totals: the lists hold the newest commits only, and
  // older ones upstream already has are dropped as well.
  const moreDropped = sup.more_own_dropped ?? 0;
  const dropped = sup.own_commits.filter((c) => c.already_upstream).length + moreDropped;
  const kept = Math.max(0, sup.ahead - dropped);
  const incomingTotal = sup.behind;
  const n = (k: number, one: string, many: string) => `${k} ${k === 1 ? one : many}`;
  const droppedNote = dropped > 0 && (
    <>
      {" "}
      — {n(dropped, "commit of yours is", "commits of yours are")} dropped because {sup.upstream} already has {dropped === 1 ? "it" : "them"}
    </>
  );
  return (
    <div aria-label="Update preview" className="rounded-lg border border-zinc-800 bg-zinc-900/40 p-3 text-xs space-y-3">
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        <Rail
          title={`You · ${sup.branch}`}
          tone="you"
          commits={sup.own_commits}
          more={sup.more_own ?? 0}
          moreNote={moreDropped > 0 ? `${moreDropped} of them already in ${sup.upstream}` : undefined}
          base={sup.base_short ?? ""}
        />
        <Rail title={sup.upstream} tone="in" commits={sup.incoming} more={sup.more_incoming ?? 0} base={sup.base_short ?? ""} />
      </div>
      <div className="rounded border border-zinc-800 bg-zinc-950/40 p-2 space-y-1">
        <p className="text-zinc-200 font-medium">After Update now</p>
        {incomingTotal === 0 ? (
          // Level with upstream: the update does not rebase the branch at all.
          <p className="text-zinc-300">
            Your branch is already level with {sup.upstream}: it is not touched and no commit id changes. Only the submodules below are
            moved. Nothing is pushed.
          </p>
        ) : kept === 0 ? (
          <p className="text-zinc-300">
            <Dot tone="in" /> Your branch moves forward to {sup.upstream}&apos;s {n(incomingTotal, "new commit", "new commits")} — there is
            nothing of yours to put back on top{droppedNote}. Nothing is pushed.
          </p>
        ) : (
          <>
            <p className="text-zinc-300">
              <Dot tone="in" /> {sup.upstream}&apos;s {n(incomingTotal, "new commit", "new commits")} below, then <Dot tone="you" /> your{" "}
              {n(kept, "commit", "commits")} on top, with new ids{droppedNote}.
            </p>
            <p className="text-zinc-400">
              Your {n(kept, "commit is", "commits are")} put back on top of the {n(incomingTotal, "new one", "new ones")}. Their ids change;
              what they change does not. Nothing is pushed.
            </p>
          </>
        )}
        {sup.will_stash && (
          <p className="text-amber-200/80">
            Your {n(sup.dirty_count, "uncommitted change is", "uncommitted changes are")} set aside first and put back afterwards.
          </p>
        )}
        {sup.file_overlap.length > 0 && (
          <p className="text-amber-200/80">
            If both sides changed the same lines of {sup.file_overlap.slice(0, 3).join(", ")}
            {sup.file_overlap.length > 3 ? ` and ${sup.file_overlap.length - 3} more` : ""}, the update pauses there and hands you the
            conflict to resolve on the Changes tab.
          </p>
        )}
      </div>
    </div>
  );
}
