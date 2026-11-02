import { useEffect, useState } from "react";
import { GitPullRequest, Loader2 } from "lucide-react";
import * as api from "../lib/api";
import { usePersistedState } from "../lib/persist";
import { GitHubIcon } from "../components/GitHubIcon";
import { Input } from "../components/Input";
import { Select } from "../components/Select";
import { PullList } from "../components/github/PullList";
import { PullPanel } from "../components/github/PullPanel";
import { Segmented, cleanError } from "../components/github/shared";

const SCOPES: { value: api.PullScope; label: string }[] = [
  { value: "created", label: "Created by me" },
  { value: "review_requested", label: "Review requested" },
  { value: "assigned", label: "Assigned" },
  { value: "involves", label: "Involving me" },
];

/**
 * Pull requests across every repository one of your accounts works on:
 * the ones it opened, the ones waiting for its review, assigned to it, or
 * involving it — open, merged, closed or all. A row opens it to review and
 * merge, as that account.
 */
export function Pulls() {
  const [accounts, setAccounts] = useState<api.RepoAccount[] | null>(null);
  const [accountsError, setAccountsError] = useState<string | null>(null);
  const [account, setAccount] = usePersistedState("pulls.account", "");
  const [scope, setScope] = usePersistedState<api.PullScope>("pulls.scope", "review_requested");
  const [state, setState] = usePersistedState<api.PullState>("pulls.state", "open");
  const [repoText, setRepoText] = useState("");
  const [repo, setRepo] = useState<string | null>(null);
  const [open, setOpen] = useState<api.PullItem | null>(null);
  const [reload, setReload] = useState(0);

  const readAccounts = () =>
    api
      .repoAccounts()
      .then((list) => {
        setAccounts(list);
        if (list.length && !list.some((a) => a.key === account)) setAccount(list[0].key);
      })
      .catch((e) => {
        setAccounts([]);
        setAccountsError(cleanError(e));
      });
  useEffect(() => {
    void readAccounts();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const repoOk = !repoText.trim() || /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(repoText.trim());
  const applyRepo = () => repoOk && setRepo(repoText.trim() || null);
  const ready = accounts && accounts.some((a) => a.key === account);

  return (
    <div className="max-w-6xl mx-auto space-y-4">
      <div>
        <h1 className="text-2xl font-bold text-zinc-100 flex items-center gap-2">
          <GitPullRequest size={22} className="text-emerald-400" />
          Pull requests
        </h1>
        <p className="text-sm text-zinc-400 mt-1">Across every repository your account works on. Open one to see its changes, review it and merge it.</p>
      </div>

      <div className="flex items-end gap-3 flex-wrap">
        <div className="w-60">
          <label className="block text-xs text-zinc-500 mb-1">Account</label>
          <Select
            value={account}
            onChange={setAccount}
            placeholder={accounts === null ? "Loading accounts…" : "No GitHub account signed in"}
            optionIcon={<GitHubIcon size={14} />}
            options={(accounts ?? []).map((a) => ({ value: a.key, label: `${a.label} · ${a.source}` }))}
          />
        </div>
        <div className="w-64">
          <label className="block text-xs text-zinc-500 mb-1">Only in repository</label>
          <Input
            value={repoText}
            onChange={(e) => setRepoText(e.target.value)}
            onBlur={applyRepo}
            onKeyDown={(e) => e.key === "Enter" && applyRepo()}
            placeholder="owner/name (optional)"
            aria-label="Only in repository"
            error={repoOk ? undefined : "Write it as owner/name"}
            className="!py-1.5 text-sm"
          />
        </div>
      </div>

      {accounts === null && (
        <p className="flex items-center gap-2 text-sm text-zinc-400" role="status">
          <Loader2 size={14} className="animate-spin text-emerald-400" /> Reading your GitHub accounts…
        </p>
      )}
      {accountsError && <p role="alert" className="text-sm text-red-400">{accountsError}</p>}
      {accounts && accounts.length === 0 && !accountsError && <p className="text-sm text-amber-300/90">Sign in on the GitHub Auth page (or with `gh auth login`) to see pull requests.</p>}

      {ready && (
        <PullList
          account={account}
          source={{ kind: "search", scope, repo }}
          state={state}
          onState={setState}
          onOpen={setOpen}
          reloadKey={reload}
          onRefresh={() => void readAccounts()}
          toolbar={<Segmented label="Whose" value={scope} onChange={setScope} options={SCOPES} />}
        />
      )}

      {open && <PullPanel account={account} repo={open.repo} number={open.number} onClose={() => setOpen(null)} onChanged={() => setReload((n) => n + 1)} />}
    </div>
  );
}
