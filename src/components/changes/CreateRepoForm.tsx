import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AlertTriangle, Building2, Loader2, User } from "lucide-react";
import { Button } from "../Button";
import { Checkbox } from "../Checkbox";
import { Input } from "../Input";
import { Select } from "../Select";
import { cleanError } from "../github/shared";
import * as api from "../../lib/api";

/**
 * A folder's name as GitHub takes it, by the same rule as the backend's
 * `repo_name_from`: anything but letters, digits, '_' and '.' becomes '-',
 * runs of '-' become one, none at the ends, no ".git" at the end, at most 100.
 * Empty when nothing usable is left.
 */
export function repoNameFrom(raw: string): string {
  let out = "";
  for (const ch of raw) {
    const c = /^[A-Za-z0-9_.]$/.test(ch) ? ch : "-";
    if (c === "-" && out.endsWith("-")) continue;
    out += c;
  }
  let name = out.replace(/^-+|-+$/g, "");
  while (name.toLowerCase().endsWith(".git")) name = name.slice(0, -4).replace(/-+$/, "");
  name = name.slice(0, 100).replace(/^-+|-+$/g, "");
  return name === "." || name === ".." ? "" : name;
}

type Props = {
  repoPath: string;
  context: api.PublishContext;
  /** "owner/name" to start from: a repository a push or the address check found missing. */
  prefill?: string | null;
  /** Created on GitHub: the dialog connects and publishes (or says what stands in the way). */
  onCreated: (published: api.Published) => void;
  /** Publish to an existing repository instead: the address tab, filled in. */
  onUseAddress: (url: string) => void;
  /** While GitHub is asked, the dialog can't be closed. */
  onBusy: (busy: boolean) => void;
};

/**
 * Create the repository on GitHub without leaving GitSwitch: as one of its
 * accounts, under that account or one of its organizations, empty (no
 * README, licence or .gitignore), private unless the person says otherwise.
 */
export function CreateRepoForm({ repoPath, context, prefill, onCreated, onUseAddress, onBusy }: Props) {
  const navigate = useNavigate();
  const [prefillOwner, prefillName] = (prefill ?? "").split("/");
  const [account, setAccount] = useState(context.suggested ?? context.accounts[0]?.account ?? "");
  const [owners, setOwners] = useState<api.RepoOwner[] | null>(null);
  const [ssoHidden, setSsoHidden] = useState(0);
  const [ownersError, setOwnersError] = useState<string | null>(null);
  const [owner, setOwner] = useState("");
  const [rawName, setRawName] = useState(prefillName || context.default_name);
  const [description, setDescription] = useState("");
  const [isPrivate, setIsPrivate] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [existing, setExisting] = useState<string | null>(null);
  const sending = useRef(false);
  const cache = useRef(new Map<string, api.RepoOwners>());
  const asked = useRef(0);
  const nameRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    nameRef.current?.focus();
  }, []);

  // The chosen account's owners: asked once per account, a late answer for
  // an account no longer chosen ignored.
  useEffect(() => {
    if (!account) return;
    const ticket = ++asked.current;
    const self: api.RepoOwner[] = [{ login: account.split(":")[1] ?? account, kind: "user" }];
    const pick = (list: api.RepoOwner[]) => {
      const wanted = prefillOwner ? list.find((o) => o.login.toLowerCase() === prefillOwner.toLowerCase()) : undefined;
      setOwner((wanted ?? list[0])?.login ?? "");
    };
    const cached = cache.current.get(account);
    if (cached) {
      setOwners(cached.owners);
      setSsoHidden(cached.sso_hidden);
      setOwnersError(null);
      pick(cached.owners);
      return;
    }
    setOwners(null);
    setOwnersError(null);
    api.githubOwners(account).then(
      (r) => {
        if (ticket !== asked.current) return;
        cache.current.set(account, r);
        setOwners(r.owners);
        setSsoHidden(r.sso_hidden);
        pick(r.owners);
      },
      (e) => {
        if (ticket !== asked.current) return;
        setOwnersError(cleanError(e));
        setOwners(self);
        pick(self);
      }
    );
  }, [account]); // eslint-disable-line react-hooks/exhaustive-deps

  const name = repoNameFrom(rawName);
  const accountLogin = account.split(":")[1] ?? "";
  const ownerKind = owners?.find((o) => o.login === owner)?.kind ?? "user";
  const pushLogin = context.push_login;
  const lower = (s: string) => s.toLowerCase();
  const mismatch =
    pushLogin && owner
      ? ownerKind === "user"
        ? lower(owner) !== lower(pushLogin)
        : lower(accountLogin) !== lower(pushLogin)
      : false;
  const target: api.CreateTarget | null =
    context.target.kind === "add"
      ? { kind: "add" }
      : context.target.kind === "change" && context.target.remote && context.target.url
        ? { kind: "change", remote: context.target.remote, from: context.target.url }
        : null;
  const blocked = context.blocked ?? (target ? null : `This folder already publishes to ${context.target.remote ?? "its remote"}.`);
  const prefillMissing = !!prefillOwner && owners !== null && !owners.some((o) => lower(o.login) === lower(prefillOwner));
  const canCreate = !!account && !!owner && !!name && !blocked && !busy && owners !== null && target !== null;

  async function create() {
    if (!canCreate || sending.current || !target) return;
    sending.current = true;
    setBusy(true);
    onBusy(true);
    setError(null);
    setExisting(null);
    try {
      const published = await api.changesCreateGithubRepo(repoPath, account, owner, name, description, isPrivate, target);
      onCreated(published);
    } catch (e) {
      const text = cleanError(e);
      setError(text);
      if (text.includes("There's already a repository")) setExisting(`git@github.com:${owner}/${name}.git`);
    } finally {
      sending.current = false;
      setBusy(false);
      onBusy(false);
    }
  }

  if (context.accounts.length === 0) {
    return (
      <div className="space-y-3">
        <p className="text-sm text-zinc-300">
          GitSwitch has no GitHub account to create the repository with. Sign in on the GitHub page (with GitHub CLI or
          GitSwitch's own sign-in) and come back, or use the address of a repository made already.
        </p>
        <div className="flex flex-wrap gap-2">
          <Button type="button" onClick={() => navigate("/github")}>
            Go to the GitHub page
          </Button>
          <Button type="button" variant="ghost" onClick={() => onUseAddress("")}>
            Use an address
          </Button>
        </div>
      </div>
    );
  }

  return (
    <form
      className="space-y-3"
      aria-label="Create on GitHub"
      onSubmit={(e) => {
        e.preventDefault();
        void create();
      }}
    >
      {/* One under the other: a login and an organization name each get the whole width. */}
      <div className="space-y-3">
        <div className="space-y-1.5">
          <span className="block text-sm font-medium text-zinc-300">Account</span>
          <Select
            value={account}
            onChange={setAccount}
            disabled={busy}
            searchLabel="Find an account"
            options={context.accounts.map((a) => ({
              value: a.account,
              label: a.login,
              note: pushLogin && lower(a.login) === lower(pushLogin) ? `${a.source} · this folder's` : a.source,
            }))}
          />
        </div>
        <div className="space-y-1.5">
          <span className="block text-sm font-medium text-zinc-300">Owner</span>
          <Select
            value={owner}
            onChange={setOwner}
            disabled={busy || owners === null}
            placeholder={owners === null ? "Loading…" : "Pick an owner"}
            searchLabel="Find an organization"
            options={(owners ?? []).map((o) => ({
              value: o.login,
              label: o.login,
              note: o.kind === "user" ? "you" : "organization",
              icon: o.kind === "user" ? <User size={14} /> : <Building2 size={14} />,
            }))}
          />
        </div>
      </div>
      {ssoHidden > 0 && (
        <p className="text-xs text-zinc-500">
          {ssoHidden} organization{ssoHidden === 1 ? " is" : "s are"} hidden: this login isn't authorized for {ssoHidden === 1 ? "its" : "their"} single sign-on.
        </p>
      )}
      {ownersError && <p className="text-xs text-amber-300 break-words">Couldn't list the organizations ({ownersError}). You can still create it under your own account.</p>}
      {prefillMissing && <p className="text-xs text-amber-300">The address named {prefillOwner}, which this account can't create repositories in. Pick another account or owner.</p>}
      <Input
        ref={nameRef}
        label="Repository name"
        aria-label="Repository name"
        value={rawName}
        onChange={(e) => setRawName(e.target.value)}
        disabled={busy}
        spellCheck={false}
        autoComplete="off"
        error={rawName.trim() && !name ? "Use letters, digits, '-', '_' or '.'" : undefined}
      />
      {name && name !== rawName.trim() && (
        <p className="text-xs text-zinc-400">
          Will be created as <span className="font-mono text-zinc-200">{name}</span>.
        </p>
      )}
      <Input
        label="Description (optional)"
        aria-label="Description"
        value={description}
        onChange={(e) => setDescription(e.target.value)}
        disabled={busy}
        maxLength={350}
      />
      <label className="flex items-center gap-2 text-sm text-zinc-300 cursor-pointer w-fit">
        <Checkbox checked={isPrivate} onChange={setIsPrivate} disabled={busy} aria-label="Keep this code private" />
        Keep this code private
      </label>
      {mismatch && (
        <p className="flex gap-2 text-xs text-amber-300">
          <AlertTriangle size={14} className="shrink-0 mt-0.5" />
          {ownerKind === "user"
            ? `This folder pushes as @${pushLogin}, and a repository under @${owner} won't accept its pushes. Pick @${pushLogin}, or change the folder's profile.`
            : `This folder pushes as @${pushLogin}, but @${accountLogin} creates it: @${pushLogin} needs access to the new repository before the publish works.`}
        </p>
      )}
      {context.branch && (
        <p className="text-xs text-zinc-500">
          <span className="font-mono">{context.branch}</span> becomes its default branch. Nothing is added on GitHub (no README,
          licence or .gitignore), so this branch's history is all it holds.
        </p>
      )}
      {blocked && (
        <p role="alert" className="px-3 py-2 rounded-lg bg-amber-500/10 border border-amber-500/30 text-amber-300 text-xs break-words">
          {blocked}
        </p>
      )}
      {error && (
        <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
          {error}
        </p>
      )}
      {existing && (
        <Button type="button" size="sm" variant="secondary" onClick={() => onUseAddress(existing)}>
          Publish to it
        </Button>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <button
          type="button"
          className="text-xs text-zinc-500 hover:text-zinc-300 hover:underline cursor-pointer disabled:cursor-default disabled:no-underline"
          disabled={!owner || !name}
          onClick={() =>
            void openUrl(`https://github.com/new?owner=${owner}&name=${name}&visibility=${isPrivate ? "private" : "public"}`).catch(() => undefined)
          }
        >
          or create it on github.com
        </button>
        <Button type="submit" disabled={!canCreate}>
          {busy ? (
            <>
              <Loader2 size={14} className="animate-spin" /> Creating…
            </>
          ) : (
            "Create and publish"
          )}
        </Button>
      </div>
      {busy && (
        <p role="status" className="sr-only">
          Creating the repository on GitHub…
        </p>
      )}
    </form>
  );
}
