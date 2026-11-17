import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AlertTriangle, Building2, ExternalLink, Globe, Loader2, Lock, User } from "lucide-react";
import { Button } from "../Button";
import { Input } from "../Input";
import { Select } from "../Select";
import { cleanError } from "../github/shared";
import { cachedOwners, loadOwners } from "./publishCache";
import * as api from "../../lib/api";
import { lastChangeTo } from "../../lib/repoChanges";

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
  /** The accounts aren't read yet (a moment: local files only). */
  accountsPending?: boolean;
  /** Where it would be connected isn't known yet: the folder is being read. */
  targetPending?: boolean;
  /** Reading the folder or its accounts failed: said here; the create command checks for itself. */
  problem?: string | null;
  /** "owner/name" to start from: a repository a push or the address check found missing. */
  prefill?: string | null;
  /**
   * "owner/name" GitHub lost (deleted here, or a push found nothing there),
   * to make again. When the folder's address names exactly that repository
   * (target "again"), the owner and name are fixed, the address is kept, and
   * the folder publishes as before. A folder that turns out never to have
   * reached its remote ("change"), or to have none ("add"), gets the usual
   * form, filled in with it.
   */
  again?: string | null;
  /** Created on GitHub: the dialog connects and publishes (or says what stands in the way). */
  onCreated: (published: api.Published) => void;
  /** Publish to an existing repository instead: the address tab, filled in. */
  onUseAddress: (url: string) => void;
  /** While GitHub is asked, the dialog can't be closed. */
  onBusy: (busy: boolean) => void;
  onCancel?: () => void;
};

const lower = (s: string) => s.toLowerCase();
const loginOf = (key: string) => key.slice(key.indexOf(":") + 1);

/**
 * Create the repository on GitHub without leaving GitSwitch: as one of its
 * accounts, under that account or one of its organizations, empty (no
 * README, licence or .gitignore), private unless the person says otherwise.
 * Drawn and usable at once: the account's own name is always a place to
 * create it, so its organizations are listed in the background.
 */
export function CreateRepoForm({
  repoPath,
  context,
  accountsPending = false,
  targetPending = false,
  problem = null,
  prefill,
  again = null,
  onCreated,
  onUseAddress,
  onBusy,
  onCancel,
}: Props) {
  const navigate = useNavigate();
  const [prefillOwner = "", prefillName = ""] = (prefill ?? "").split("/");
  /** Making it again: the account that is its owner, when GitSwitch has it, creates it. */
  const ownersAccount = (c: api.PublishContext) => (again && prefillOwner ? c.accounts.find((a) => lower(a.login) === lower(prefillOwner))?.account : undefined);
  const startAccount = ownersAccount(context) ?? context.suggested ?? context.accounts[0]?.account ?? "";
  const [account, setAccount] = useState(startAccount);
  const [orgs, setOrgs] = useState<api.RepoOwners | null>(() => (startAccount ? cachedOwners(startAccount) : null));
  const [ownersError, setOwnersError] = useState<string | null>(null);
  const [ownerPick, setOwner] = useState(prefillOwner || loginOf(startAccount));
  const [rawName, setRawName] = useState(prefillName || context.default_name);
  const [description, setDescription] = useState("");
  const [isPrivate, setIsPrivate] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [existing, setExisting] = useState<string | null>(null);
  const sending = useRef(false);
  /** The person picked an account: answers that come in later don't move it. */
  const picked = useRef(false);
  const asked = useRef(0);
  const nameRef = useRef<HTMLInputElement>(null);
  const formRef = useRef<HTMLFormElement>(null);

  useEffect(() => {
    // The name box; with the name fixed (made again), the first control there is.
    (nameRef.current ?? formRef.current?.querySelector<HTMLElement>("button:not(:disabled), input:not(:disabled)"))?.focus();
  }, []);

  // Accounts read after the form was drawn, or the folder's own login found
  // later (its key, asked of github.com): the account follows them until the
  // person picks one.
  const accountsKey = context.accounts.map((a) => a.account).join("\n");
  useEffect(() => {
    if (picked.current) return;
    const stillThere = context.accounts.some((a) => a.account === account);
    const next = ownersAccount(context) ?? context.suggested ?? (stillThere ? account : (context.accounts[0]?.account ?? ""));
    if (next !== account) setAccount(next);
  }, [context.suggested, accountsKey]); // eslint-disable-line react-hooks/exhaustive-deps

  // The chosen account's organizations, in the background (kept ten minutes).
  // The owner starts as the address's owner, else the account itself, which
  // is always a place to create it; a late answer for another account is
  // ignored.
  useEffect(() => {
    if (!account) return;
    const ticket = ++asked.current;
    const self = loginOf(account);
    const hit = cachedOwners(account);
    setOrgs(hit);
    setOwnersError(null);
    const keep = (list: api.RepoOwner[], cur: string) => (list.some((o) => lower(o.login) === lower(cur)) ? cur : self);
    setOwner(hit ? keep(hit.owners, prefillOwner || self) : prefillOwner || self);
    if (hit) return;
    loadOwners(account).then(
      (r) => {
        if (ticket !== asked.current) return;
        setOrgs(r);
        setOwner((cur) => keep(r.owners, cur));
      },
      (e) => {
        if (ticket !== asked.current) return;
        setOwnersError(cleanError(e));
        setOwner(self);
      }
    );
  }, [account]); // eslint-disable-line react-hooks/exhaustive-deps

  // Made again: the owner and the name are the address's, never picked. Fixed
  // while the folder is read too, unless it turns out to need the usual form.
  const fixed = !!again && context.target.kind !== "add" && context.target.kind !== "change";
  const owner = fixed ? prefillOwner : ownerPick;
  const name = fixed ? prefillName : repoNameFrom(rawName);
  const accountLogin = account ? loginOf(account) : "";
  const loadingOrgs = !!account && orgs === null && !ownersError;
  // Until GitHub lists the organizations: the account itself, and the
  // address's owner while it is being checked.
  const ownerList: api.RepoOwner[] = orgs?.owners ?? [
    ...(accountLogin ? [{ login: accountLogin, kind: "user" as const }] : []),
    ...(loadingOrgs && prefillOwner && lower(prefillOwner) !== lower(accountLogin) ? [{ login: prefillOwner, kind: "org" as const }] : []),
  ];
  const ownerIsSelf = !!owner && lower(owner) === lower(accountLogin);
  const ownerListed = !!orgs && orgs.owners.some((o) => lower(o.login) === lower(owner));
  // A repository is created under the account itself or an organization of it.
  const ownerKind = fixed ? (ownerIsSelf ? "user" : "org") : (ownerList.find((o) => lower(o.login) === lower(owner))?.kind ?? "user");
  /**
   * Made again under an organization this account's list doesn't show, while
   * GitHub hides some of its organizations behind single sign-on: it may be
   * one of them, so GitHub decides when it is created.
   */
  const ssoMaybe = fixed && !!owner && !ownerIsSelf && !!orgs && !ownerListed && (orgs.sso_hidden ?? 0) > 0;
  // Fixed, with the organizations not listed: GitHub says whether it can.
  const ownerReady = ownerIsSelf || ownerListed || (fixed && (!!ownersError || ssoMaybe));
  /** Made again under an owner this account can't create repositories in. */
  const cantCreateHere = fixed && !!account && !!owner && !ownerIsSelf && orgs !== null && !ownerListed && !ssoMaybe;
  const pushLogin = context.push_login;
  const mismatch =
    pushLogin && owner
      ? ownerKind === "user"
        ? lower(owner) !== lower(pushLogin)
        : lower(accountLogin) !== lower(pushLogin)
      : false;
  const t = context.target;
  const target: api.CreateTarget | null =
    t.kind === "add"
      ? { kind: "add" }
      : t.kind === "change" && t.remote && t.url
        ? { kind: "change", remote: t.remote, from: t.url }
        : t.kind === "again" && t.remote && t.url
          ? { kind: "again", remote: t.remote, from: t.url }
          : null;
  const where = t.repo ?? t.url;
  // Made again, but the folder publishes somewhere else: said, with the way that is left.
  const elsewhere = again
    ? t.remote
      ? `This folder publishes to ${t.remote}${where ? ` (${where})` : ""}, not ${again}, so it can't be made again from here. To create it anyway, use the link below.`
      : `GitSwitch can't tell which remote this folder publishes to, so it can't make ${again} again here. To create it anyway, use the link below.`
    : `This folder already publishes to ${t.remote ?? "its remote"}.`;
  const blocked = context.blocked ?? (target || targetPending ? null : elsewhere);
  /**
   * Made again where the folder's address points, as the folder said once it
   * was read (never while it is being read): said as such.
   */
  const againShown = t.kind === "again" && !blocked;
  /** Made again, but the folder publishes elsewhere: none of the form could be used, so it isn't drawn. */
  const deadEnd = fixed && !context.blocked && !targetPending && target === null;
  /**
   * Another of GitSwitch's accounts that may create it there: the owner
   * itself, a member of that organization, or one whose organizations
   * haven't been read yet. One known not to can't be the way forward.
   */
  const otherCould = context.accounts.some((a) => {
    if (a.account === account) return false;
    if (lower(loginOf(a.account)) === lower(owner)) return true;
    const known = cachedOwners(a.account);
    return !known || known.owners.some((o) => lower(o.login) === lower(owner));
  });
  /**
   * What GitSwitch did to it in this session, when it deleted it: whether
   * GitHub can restore that one, and where. Nothing known: said with care.
   */
  const lastChange = again ? lastChangeTo(again) : null;
  const deletedHere = lastChange?.kind === "deleted" ? lastChange : null;
  const prefillMissing = !fixed && !!prefillOwner && orgs !== null && !orgs.owners.some((o) => lower(o.login) === lower(prefillOwner));
  const waiting = accountsPending
    ? "Reading this folder's accounts…"
    : targetPending
      ? fixed
        ? "Checking this folder's address…"
        : "Checking where it would be connected…"
      : account && owner && !ownerReady && loadingOrgs
        ? `Checking that @${accountLogin} can create repositories in ${owner}…`
        : null;
  const canCreate = !!account && !!owner && ownerReady && !!name && !blocked && !busy && target !== null && !waiting;

  // The form was drawn while the folder was read, and the field with the
  // focus went with it: the focus goes to Cancel, the way out.
  useEffect(() => {
    if (deadEnd && !formRef.current?.contains(document.activeElement)) formRef.current?.querySelector<HTMLElement>("[data-create-cancel]")?.focus();
  }, [deadEnd]);

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

  if (!accountsPending && context.accounts.length === 0) {
    return (
      <div className="space-y-3">
        <p className="text-sm text-zinc-300">
          GitSwitch has no GitHub account to create the repository with. Sign in on the GitHub Auth page (with GitHub CLI or
          GitSwitch's own sign-in) and come back{fixed ? "." : ", or use the address of a repository made already."}
        </p>
        {problem && <p className="text-xs text-amber-300 break-words">{problem}</p>}
        <div className="flex flex-wrap gap-2">
          <Button type="button" onClick={() => navigate("/github")}>
            Go to GitHub Auth
          </Button>
          {/* An address is for a folder that has no remote yet, or one never reached: not for one made again where it points. */}
          {!fixed && (
            <Button type="button" variant="ghost" onClick={() => onUseAddress("")}>
              Use an address
            </Button>
          )}
        </div>
      </div>
    );
  }

  const visibilities = [
    {
      value: true,
      label: "Private",
      icon: <Lock size={14} />,
      text:
        ownerKind === "org"
          ? `Only the people you or ${owner} give access to can see it.`
          : "Only you and the people you invite can see it.",
    },
    {
      value: false,
      label: "Public",
      icon: <Globe size={14} />,
      text: "Anyone on the internet can see it. Only the people you invite can change it.",
    },
  ];

  return (
    <form
      ref={formRef}
      className="space-y-3"
      aria-label={fixed ? "Create it again on GitHub" : "Create on GitHub"}
      data-create-mode={fixed ? "again" : "new"}
      onSubmit={(e) => {
        e.preventDefault();
        void create();
      }}
    >
      {againShown && (
        <>
          <p className="text-sm text-zinc-300" data-again-intro>
            GitHub has no repository at this folder's address any more, or @{pushLogin ?? accountLogin} can't see it. Create it
            again under the same name: this folder's address stays as it is.
          </p>
          {/* Not an undo: said before anything is created, as a name taken again can't be restored into —
              and only what is true of the one deleted, when GitSwitch deleted it in this session. */}
          <div className="space-y-1" data-again-new>
            <p className="text-xs text-zinc-400">
              It is a new, empty repository: only this folder's commits go to it.{" "}
              {deletedHere
                ? deletedHere.restoreUrl
                  ? "Deleted by mistake? Restore it instead (GitHub keeps it for 90 days): that brings back its issues and pull requests too."
                  : "The one deleted can't be restored from settings: it was part of a fork network."
                : "If it was deleted by mistake, GitHub may still keep it for 90 days: restoring it brings back its issues and pull requests too (not for a fork network; an organization's is restored by its owners)."}
            </p>
            {deletedHere?.restoreUrl && (
              <button
                type="button"
                className="text-xs text-zinc-400 hover:text-zinc-200 hover:underline cursor-pointer inline-flex items-center gap-1"
                onClick={() => void openUrl(deletedHere.restoreUrl!).catch(() => undefined)}
                data-again-restore
              >
                <ExternalLink size={12} /> Restore it on github.com
              </button>
            )}
          </div>
        </>
      )}
      {/* Made again where the folder doesn't publish: only why, and the way that is left. */}
      {!deadEnd && (
        <>
          {/* One under the other: a login and an organization name each get the whole width. */}
          <div className="space-y-3">
            <div className="space-y-1.5">
              <span className="block text-sm font-medium text-zinc-300">Account</span>
              <Select
                value={account}
                onChange={(v) => {
                  picked.current = true;
                  setAccount(v);
                }}
                disabled={busy || accountsPending}
                placeholder={accountsPending ? "Loading…" : "Pick an account"}
                searchLabel="Find an account"
                options={context.accounts.map((a) => ({
                  value: a.account,
                  label: a.login,
                  note: pushLogin && lower(a.login) === lower(pushLogin) ? `${a.source} · this folder's` : a.source,
                }))}
              />
            </div>
            {fixed ? (
              <div className="space-y-1.5">
                <span className="block text-sm font-medium text-zinc-300">Repository</span>
                <p
                  className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 px-3 py-2 rounded-lg bg-zinc-800/60 border border-zinc-700/60 text-sm"
                  data-again-repo={`${owner}/${name}`}
                >
                  <span className="font-mono text-zinc-100 break-all">{`${owner}/${name}`}</span>
                  {againShown && <span className="text-xs text-zinc-500">this folder's address</span>}
                </p>
                {loadingOrgs && !ownerIsSelf && (
                  <p className="flex items-center gap-1.5 text-xs text-zinc-500">
                    <Loader2 size={12} className="animate-spin" /> Listing your organizations…
                  </p>
                )}
              </div>
            ) : (
              <div className="space-y-1.5">
                <span className="block text-sm font-medium text-zinc-300">Owner</span>
                <Select
                  value={owner}
                  onChange={setOwner}
                  disabled={busy || !account}
                  placeholder={accountsPending ? "Loading…" : "Pick an owner"}
                  searchLabel="Find an organization"
                  options={ownerList.map((o) => ({
                    value: o.login,
                    label: o.login,
                    note: o.kind === "user" ? "you" : "organization",
                    icon: o.kind === "user" ? <User size={14} /> : <Building2 size={14} />,
                  }))}
                />
                {loadingOrgs && (
                  <p className="flex items-center gap-1.5 text-xs text-zinc-500">
                    <Loader2 size={12} className="animate-spin" /> Listing your organizations…
                  </p>
                )}
              </div>
            )}
          </div>
          {!!orgs?.sso_hidden && (
            <p className="text-xs text-zinc-500">
              {orgs.sso_hidden} organization{orgs.sso_hidden === 1 ? " is" : "s are"} hidden: this login isn't authorized for{" "}
              {orgs.sso_hidden === 1 ? "its" : "their"} single sign-on.
            </p>
          )}
          {ownersError && !fixed && (
            <p className="text-xs text-amber-300 break-words">
              Couldn't list the organizations ({ownersError}). You can still create it under your own account.
            </p>
          )}
          {ownersError && fixed && !ownerIsSelf && (
            <p className="text-xs text-amber-300 break-words">
              Couldn't list @{accountLogin}'s organizations ({ownersError}). GitHub says whether it can create repositories in {owner} when
              you create it.
            </p>
          )}
          {cantCreateHere && (
            <div role="alert" className="space-y-2" data-cant-create-here>
              <p className="text-xs text-amber-300">
                {otherCould
                  ? `@${accountLogin} can't create repositories in ${owner}. Pick another account.`
                  : `@${accountLogin} can't create repositories in ${owner}. Sign in on the GitHub Auth page with an account that can, then come back.`}
              </p>
              {/* No other account that may: the way on is another sign-in. */}
              {!otherCould && (
                <Button type="button" size="sm" variant="secondary" onClick={() => navigate("/github")}>
                  Go to GitHub Auth
                </Button>
              )}
            </div>
          )}
          {ssoMaybe && (
            <p className="text-xs text-zinc-400" data-sso-maybe>
              {owner} isn't among @{accountLogin}'s organizations as GitHub lists them, but it may be one of those hidden by single
              sign-on: GitHub says whether @{accountLogin} can create it there when you create it.
            </p>
          )}
          {prefillMissing && (
            <p className="text-xs text-amber-300">
              The address named {prefillOwner}, which this account can't create repositories in. Pick another account or owner.
            </p>
          )}
          {!fixed && (
            <>
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
            </>
          )}
          <Input
            label="Description (optional)"
            aria-label="Description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            disabled={busy}
            maxLength={350}
          />
          <fieldset className="space-y-1.5" disabled={busy}>
            <legend className="block text-sm font-medium text-zinc-300 mb-1.5">Who can see it</legend>
            <div className="grid grid-cols-2 gap-2">
              {visibilities.map((v) => {
                const on = isPrivate === v.value;
                return (
                  <label
                    key={v.label}
                    className={`flex flex-col gap-1 rounded-lg border px-3 py-2 cursor-pointer transition-colors focus-within:ring-2 focus-within:ring-blue-500/60 ${
                      on ? "border-blue-500/70 bg-blue-500/10" : "border-zinc-700/60 hover:border-zinc-600 bg-zinc-800/40"
                    }`}
                  >
                    <input
                      type="radio"
                      name="visibility"
                      className="sr-only"
                      checked={on}
                      onChange={() => setIsPrivate(v.value)}
                      aria-label={`${v.label}: ${v.text}`}
                    />
                    <span className={`flex items-center gap-1.5 text-sm font-medium ${on ? "text-zinc-100" : "text-zinc-300"}`}>
                      {v.icon} {v.label}
                    </span>
                    <span className="text-xs text-zinc-400 leading-snug">{v.text}</span>
                  </label>
                );
              })}
            </div>
            <p className="text-xs text-zinc-500">You can change this later.</p>
          </fieldset>
          {!isPrivate && (
            <p className="flex gap-2 text-xs text-amber-300">
              <AlertTriangle size={14} className="shrink-0 mt-0.5" />
              Everything in this branch's history becomes visible, even files deleted in earlier commits. Make sure it holds no
              passwords, keys or private files.
            </p>
          )}
          {mismatch && (
            <p className="flex gap-2 text-xs text-amber-300">
              <AlertTriangle size={14} className="shrink-0 mt-0.5" />
              {ownerKind === "user"
                ? fixed
                  ? `This folder pushes as @${pushLogin}, but the repository is @${owner}'s: @${pushLogin} needs access to it before the publish works.`
                  : `This folder pushes as @${pushLogin}, and a repository under @${owner} won't accept its pushes. Pick @${pushLogin}, or change the folder's profile.`
                : `This folder pushes as @${pushLogin}, but @${accountLogin} creates it: @${pushLogin} needs access to the new repository before the publish works.`}
            </p>
          )}
        </>
      )}
      {problem && <p className="text-xs text-amber-300 break-words">{problem}</p>}
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
      {/* A name taken: publish to that repository instead — never when making one again where the address points, which has no address tab. */}
      {existing && !fixed && (
        <Button type="button" size="sm" variant="secondary" onClick={() => onUseAddress(existing)}>
          Publish to it
        </Button>
      )}
      {againShown && owner && name && (
        <p className="text-xs text-zinc-400" data-testid="create-summary">
          Creates <span className="font-mono text-zinc-200">{`${owner}/${name}`}</span> on GitHub again as a{" "}
          <span className="text-zinc-200">{isPrivate ? "private" : "public"}</span> repository, then publishes{" "}
          {context.branch ? <span className="font-mono text-zinc-200">{context.branch}</span> : "this branch"} to it. This folder's
          address stays as it is. Nothing else is added (no README, licence or .gitignore).
        </p>
      )}
      {!fixed && owner && name && (
        <p className="text-xs text-zinc-400" data-testid="create-summary">
          Creates <span className="font-mono text-zinc-200">{`${owner}/${name}`}</span> on GitHub as a{" "}
          <span className="text-zinc-200">{isPrivate ? "private" : "public"}</span> repository, then publishes{" "}
          {context.branch ? <span className="font-mono text-zinc-200">{context.branch}</span> : "this branch"} to it. Nothing else is
          added (no README, licence or .gitignore).
        </p>
      )}
      {waiting && !deadEnd && (
        <p role="status" className="flex items-center gap-1.5 text-xs text-zinc-500">
          <Loader2 size={12} className="animate-spin" /> {waiting}
        </p>
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
          {/* The only way left when the form isn't drawn; a fallback beside it otherwise. */}
          {deadEnd ? "Create it on github.com" : "or create it on github.com"}
        </button>
        <div className="flex gap-2">
          {onCancel && (
            <Button type="button" variant="ghost" onClick={onCancel} disabled={busy} data-create-cancel>
              Cancel
            </Button>
          )}
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
      </div>
      {busy && (
        <p role="status" className="sr-only">
          Creating the repository on GitHub…
        </p>
      )}
    </form>
  );
}
