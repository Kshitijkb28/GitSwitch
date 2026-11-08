import { useEffect, useRef, useState } from "react";
import { AlertTriangle, ExternalLink, Loader2 } from "lucide-react";
import * as api from "../../lib/api";
import { usePersistedState } from "../../lib/persist";
import { Button } from "../Button";
import { GitHubIcon } from "../GitHubIcon";
import { Select } from "../Select";
import { Collaborators } from "../github/Collaborators";
import { PullList } from "../github/PullList";
import { PullPanel } from "../github/PullPanel";
import { Segmented, cleanError, openOnGitHub } from "../github/shared";

const PERMISSION_WORD: Record<api.AccountAccess["permission"], string> = {
  admin: "admin",
  push: "can push",
  read: "can read",
  none: "no access",
  unknown: "not checked",
};

/**
 * The open repository on GitHub: its pull requests (to review and merge) and
 * who it is shared with — as one of your accounts, the profile's own by
 * default when it can see the repository.
 */
export function GitHubTab({ repoPath, visible, reloadKey = 0 }: { repoPath: string; visible: boolean; reloadKey?: number }) {
  const [ctx, setCtx] = useState<api.RepoContext | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The account picked per repository, by "owner/name". */
  const [picked, setPicked] = usePersistedState<Record<string, string>>("github.actingAs", {});
  const [section, setSection] = usePersistedState<"pulls" | "people">("github.section", "pulls");
  const [state, setState] = usePersistedState<api.PullState>("github.pullState", "open");
  const [open, setOpen] = useState<api.PullItem | null>(null);
  const [reload, setReload] = useState(0);
  /** The repository the context was read for; a tab hidden since keeps it. */
  const readFor = useRef<string | null>(null);
  const ask = useRef(0);

  async function load() {
    const id = ++ask.current;
    readFor.current = repoPath;
    setLoading(true);
    setError(null);
    try {
      const c = await api.githubRepoContext(repoPath);
      if (id === ask.current) setCtx(c);
    } catch (e) {
      if (id === ask.current) {
        setCtx(null);
        setError(cleanError(e));
      }
    } finally {
      if (id === ask.current) setLoading(false);
    }
  }

  useEffect(() => {
    // Read only once the tab is shown, and again when another repository is picked.
    if (visible && repoPath && readFor.current !== repoPath) {
      setOpen(null);
      void load();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, repoPath]);

  // The page's Refresh: read the accounts and the list again (only once read before).
  const firstReload = useRef(reloadKey);
  useEffect(() => {
    if (reloadKey === firstReload.current) return;
    firstReload.current = reloadKey;
    if (visible && readFor.current) {
      void load();
      setReload((n) => n + 1);
    } else {
      readFor.current = null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reloadKey]);

  const slug = ctx?.owner && ctx.name ? `${ctx.owner}/${ctx.name}` : null;
  const accounts = ctx?.accounts ?? [];
  const chosen = (slug && accounts.find((a) => a.account === picked[slug.toLowerCase()])) || accounts.find((a) => a.account === ctx?.suggested) || accounts[0] || null;
  const account = chosen?.account ?? "";

  return (
    <div className={visible ? "space-y-4" : "hidden"}>
      {loading && !ctx && (
        <p className="flex items-center gap-2 text-sm text-zinc-400" role="status">
          <Loader2 size={14} className="animate-spin text-emerald-400" /> Asking GitHub which of your accounts can reach this repository…
        </p>
      )}
      {error && (
        <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs">
          {error}
        </p>
      )}
      {ctx?.not_github && <p className="text-sm text-zinc-400">{ctx.not_github} Pull requests and collaborators are for repositories on GitHub.</p>}

      {slug && (
        <>
          <div className="flex items-center gap-3 flex-wrap">
            <span className="flex items-center gap-2 text-sm text-zinc-200 font-mono min-w-0">
              <GitHubIcon size={15} />
              <span className="truncate" title={slug}>
                {slug}
              </span>
            </span>
            <Button variant="ghost" size="sm" onClick={() => openOnGitHub(`https://github.com/${slug}`)} title="Open on GitHub">
              <ExternalLink size={13} />
            </Button>
            <span className="text-xs text-zinc-500 ml-auto">Acting as</span>
            <div className="w-80 max-w-full">
              <Select
                value={account}
                onChange={(v) => setPicked((m) => ({ ...m, [slug.toLowerCase()]: v }))}
                placeholder={accounts.length ? "Pick an account" : "No GitHub account signed in"}
                optionIcon={<GitHubIcon size={14} />}
                options={accounts.map((a) => ({ value: a.account, label: `${a.login} · ${PERMISSION_WORD[a.permission]}${ctx?.profile_login?.toLowerCase() === a.login.toLowerCase() ? " · this profile" : ""}` }))}
              />
            </div>
          </div>
          {accounts.length === 0 && <p className="text-sm text-amber-300/90">Sign in on the GitHub Auth page (or with `gh auth login`) to see this repository's pull requests.</p>}
          {chosen && (chosen.permission === "none" || chosen.permission === "unknown") && (
            <p className="text-xs text-amber-300/90 flex items-center gap-2">
              <AlertTriangle size={13} className="shrink-0" />
              {chosen.permission === "none" ? `GitHub shows ${slug} to @${chosen.login} as not there: pick an account that can see it.` : `@${chosen.login}'s access could not be checked${chosen.why ? `: ${chosen.why}` : ""}.`}
            </p>
          )}

          {account && (
            <>
              <Segmented
                label="Show"
                value={section}
                onChange={setSection}
                options={[
                  { value: "pulls", label: "Pull requests" },
                  { value: "people", label: "Collaborators" },
                ]}
              />
              {section === "pulls" ? (
                <PullList account={account} source={{ kind: "repo", owner: ctx!.owner!, name: ctx!.name! }} state={state} onState={setState} onOpen={setOpen} reloadKey={reload} />
              ) : (
                <Collaborators account={account} owner={ctx!.owner!} name={ctx!.name!} admin={chosen?.permission === "admin"} />
              )}
            </>
          )}
        </>
      )}

      {open && account && <PullPanel account={account} repo={open.repo} number={open.number} onClose={() => setOpen(null)} onChanged={() => setReload((n) => n + 1)} />}
    </div>
  );
}
