import { Files, TriangleAlert } from "lucide-react";
import type { WorkspaceCopy, WorkspaceInfo } from "../../lib/api";

/** One copy, in words: "Argos_06-10-26_V1 (this one, main, 3 changed)". */
export function copyWords(c: WorkspaceCopy): string {
  const parts: string[] = [];
  if (c.is_this) parts.push("this one");
  if (c.kind === "worktree") parts.push("worktree");
  if (c.unborn) parts.push("no commits yet");
  else if (c.detached) parts.push("detached");
  else parts.push(c.branch ?? "no branch");
  if (c.changed === null) parts.push("unreadable");
  else if (c.changed === 0) parts.push("clean");
  else parts.push(`${c.changed} changed`);
  return `${c.label} (${parts.join(", ")})`;
}

/**
 * The strip's sentence: "2 copies of acme/argos among your repositories:
 * Argos_06-10-26_V1 (this one, main, 3 changed) · Argos_08-10-26 (main, clean)".
 */
export function copiesLine(info: WorkspaceInfo): string {
  return `${info.copies.length} copies of ${info.remote ?? info.name} among your repositories: ${info.copies.map(copyWords).join(" · ")}`;
}

/**
 * The other checkouts of this project on this machine, and the one warning
 * worth giving: the same branch checked out in two of them, with changes in
 * both. Nothing here runs git — the row's title says what is left to the
 * person. Hidden for a repository cloned once.
 */
export function WorkspaceCopies({ info }: { info: WorkspaceInfo | null }) {
  if (!info || info.copies.length < 2) return null;
  return (
    <>
      <p className="text-xs text-zinc-400 leading-relaxed flex items-start gap-1.5" aria-label="Copies of this repository" title={info.not_automated}>
        <Files size={13} className="text-zinc-500 shrink-0 mt-0.5" />
        <span className="break-words min-w-0">{copiesLine(info)}</span>
      </p>
      {info.note && (
        <p className="text-xs text-amber-200 leading-relaxed flex items-start gap-1.5" aria-label="Copies note">
          <TriangleAlert size={13} className="text-amber-400 shrink-0 mt-0.5" />
          <span className="break-words min-w-0">{info.note.text}</span>
        </p>
      )}
      {info.warnings.map((w) => (
        <p key={w} className="text-xs text-amber-200/80 leading-relaxed break-words">
          {w}
        </p>
      ))}
    </>
  );
}
