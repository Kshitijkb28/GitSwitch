import { useState } from "react";
import { FolderGit2, FolderOpen, Loader2, Pencil } from "lucide-react";
import { Card } from "../Card";
import { Button } from "../Button";
import { cleanError } from "../github/shared";

/** A folder on the Repository page that is no repository. */
export interface FolderEntry {
  path: string;
  name: string;
  /** The profile whose folder it is; none for a folder opened by hand. */
  profileId: string | null;
  profileName: string | null;
  profileEmail: string | null;
  /** It is inside this repository (its top folder). */
  inside: string | null;
}

/**
 * What the Repository page shows for a folder git doesn't know as a
 * repository, instead of tabs that would only fail: why it is listed, and the
 * one step that makes it a repository (or, inside one, the way to it).
 */
export function NotARepositoryCard({
  entry,
  onMake,
  onOpenRepo,
  onEditProfile,
}: {
  entry: FolderEntry;
  /** Run `git init` there; rejects with the reason when it can't. */
  onMake: () => Promise<void>;
  onOpenRepo: (top: string) => void;
  onEditProfile: (profileId: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function make() {
    setBusy(true);
    setError(null);
    try {
      await onMake();
    } catch (e) {
      setError(cleanError(e));
    } finally {
      setBusy(false);
    }
  }

  const whose = entry.profileName ? `${entry.profileName}'s` : "a profile's";
  return (
    <Card className="py-6">
      <div className="flex items-start gap-4" data-not-a-repository={entry.inside ? "inside" : "folder"}>
        <FolderGit2 size={30} className="shrink-0 text-zinc-500 mt-0.5" />
        <div className="space-y-3 min-w-0">
          <h2 className="text-lg font-medium text-zinc-100">
            {entry.name} isn’t a git repository{entry.inside ? "" : " yet"}
          </h2>
          {entry.inside ? (
            <p className="text-sm text-zinc-400">
              It is inside the repository <span className="font-mono text-zinc-300 break-all">{entry.inside}</span>, so it can’t be one of its own, and {whose} identity
              doesn’t apply to commits there: git goes by where the repository is.
            </p>
          ) : entry.profileName ? (
            <p className="text-sm text-zinc-400">
              It is one of <span className="text-zinc-200">{entry.profileName}</span>’s folders, so GitSwitch lists it. Once it is a repository, commits here use{" "}
              {entry.profileName}’s identity{entry.profileEmail ? ` (${entry.profileEmail})` : ""}.
            </p>
          ) : (
            <p className="text-sm text-zinc-400">Once it is a repository, GitSwitch shows its changes and history here.</p>
          )}
          <p className="font-mono text-xs text-zinc-500 break-all">{entry.path}</p>
          <div className="flex flex-wrap items-center gap-2">
            {entry.inside ? (
              <Button size="sm" onClick={() => entry.inside && onOpenRepo(entry.inside)} data-open-enclosing>
                <FolderOpen size={14} />
                Open that repository
              </Button>
            ) : (
              <Button size="sm" onClick={() => void make()} disabled={busy} data-make-repository>
                {busy ? <Loader2 size={14} className="animate-spin" /> : <FolderGit2 size={14} />}
                Make it a repository
              </Button>
            )}
            {entry.profileId && (
              <Button size="sm" variant="secondary" onClick={() => entry.profileId && onEditProfile(entry.profileId)}>
                <Pencil size={14} />
                Edit {entry.profileName ?? "profile"}
              </Button>
            )}
          </div>
          {!entry.inside && (
            <p className="text-xs text-zinc-500">
              This runs <span className="font-mono">git init</span> here: it adds a .git folder and changes none of your files. The first branch is main, or the name your
              git is set to use.
            </p>
          )}
          {error && (
            <p className="text-sm text-red-400 whitespace-pre-wrap" data-make-repository-error>
              {error}
            </p>
          )}
        </div>
      </div>
    </Card>
  );
}
