import { useState, useEffect, useRef, type SubmitEvent } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft, FolderPlus, X, Key, FolderOpen, Link2, Loader2, CheckCircle2, Globe } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { Card } from "../components/Card";
import { Checkbox } from "../components/Checkbox";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import type { Profile } from "../types/profile";
import { baseName } from "../lib/paths";
import * as api from "../lib/api";
import { identityOf, isSignInValue, loginOfValue, rememberedSignIns, SIGNIN_PREFIX } from "../lib/signins";

export function ProfileForm() {
  const navigate = useNavigate();
  const toast = useToast();
  const { id } = useParams<{ id: string }>();
  const [searchParams] = useSearchParams();
  const isEdit = !!id;

  const [name, setName] = useState(searchParams.get("github_name") || "");
  const [gitName, setGitName] = useState(searchParams.get("git_name") || "");
  const [gitEmail, setGitEmail] = useState(searchParams.get("github_email") || "");
  const [sshKeyPath, setSshKeyPath] = useState("");
  const [allowPush, setAllowPush] = useState(true);
  const [signingEnabled, setSigningEnabled] = useState(false);
  const [signingBusy, setSigningBusy] = useState(false);
  const [signingMsg, setSigningMsg] = useState<string | null>(null);
  const [isDefaultProfile, setIsDefaultProfile] = useState(false);
  const [directories, setDirectories] = useState<string[]>([]);
  const [dirInput, setDirInput] = useState("");
  const [remotePatterns, setRemotePatterns] = useState<string[]>([]);
  const [remoteInput, setRemoteInput] = useState("");
  // The GitHub account this profile pushes as — from the sign-in page, or the
  // account picked in "Autofill from GitHub". The Changes page checks a push
  // against it.
  const [githubLogin, setGithubLogin] = useState(searchParams.get("github_name") || "");
  const [sshKeys, setSshKeys] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [converting, setConverting] = useState(false);
  const [convertResults, setConvertResults] = useState<api.RemoteChange[] | null>(null);
  const [ghAccounts, setGhAccounts] = useState<string[]>([]);
  const [autofillAccount, setAutofillAccount] = useState("");
  const [autofilling, setAutofilling] = useState(false);
  // Working out which account a loaded profile is, in the background.
  const [preselecting, setPreselecting] = useState(false);
  /** Why only the no-reply address is offered, and how to share the others. */
  const [emailHint, setEmailHint] = useState<string | null>(null);
  const scopeHint = (login: string) =>
    `GitHub shares @${login}'s private addresses only with a token that has the user:email scope. In a terminal: gh auth switch -h github.com -u ${login}, then gh auth refresh -h github.com -s user:email. Then pick the account again.`;
  const [suggestions, setSuggestions] = useState<{ names: string[]; emails: string[] }>({
    names: [],
    emails: [],
  });
  const [originalIdentity, setOriginalIdentity] = useState<{
    name: string;
    email: string;
  } | null>(null);
  // Edit mode renders a loading state until the profile is in hand: a blank but
  // interactive form could be typed into (and submitted), and an empty
  // directories/ssh-key submit is treated by the backend as an explicit clear.
  const [loadingProfile, setLoadingProfile] = useState(isEdit);
  // Last-write-wins guard so a superseded autofill can never write stale state.
  const autofillSeq = useRef(0);
  // Once the user picks an account themselves, background pre-selection stops.
  const userPicked = useRef(false);
  // The profile name we auto-filled, so switching accounts updates it but
  // anything the user typed is left alone.
  const autoFilledName = useRef<string | null>(null);

  // Accounts signed in with GitHub on this computer: their public identity
  // only, no token — offered beside gh's logins.
  const [signins] = useState(rememberedSignIns);

  /** A gh login asks GitHub through gh's token; a remembered sign-in answers from what was kept. */
  async function fetchIdentity(account: string) {
    if (isSignInValue(account)) {
      const login = loginOfValue(account).toLowerCase();
      const saved = signins.find((s) => s.login.toLowerCase() === login);
      if (!saved) throw new Error(`${loginOfValue(account)} is no longer remembered — sign in with GitHub again.`);
      return identityOf(saved);
    }
    const token = await api.ghGetToken(account);
    return identityOf(await api.verifyGithubToken(token));
  }

  /** The user chose an account — fill the git identity fields. */
  async function applyAccountIdentity(account: string) {
    userPicked.current = true;
    const seq = ++autofillSeq.current;
    const previous = autofillAccount;
    setAutofillAccount(account);
    if (!account) {
      setSuggestions({ names: [], emails: [] });
      return;
    }
    setAutofilling(true);
    setError(null);
    try {
      const idty = await fetchIdentity(account);
      if (autofillSeq.current !== seq) return; // superseded by a newer pick
      setSuggestions({ names: idty.names, emails: idty.emails });
      // Default to the login handle — that's what git identities conventionally
      // use, and GitHub attributes commits by email regardless of the name.
      setGitName(idty.login);
      // Re-picking a profile's own account whose token cannot read its
      // addresses keeps the address the profile has, not the no-reply one.
      const own = isEdit && !!githubLogin && githubLogin.replace(/^@/, "").toLowerCase() === idty.login.toLowerCase();
      if (!(own && idty.scopeMissing)) setGitEmail(idty.email);
      setEmailHint(idty.scopeMissing ? scopeHint(idty.login) : null);
      setName((prev) => (!prev || prev === autoFilledName.current ? idty.login : prev));
      autoFilledName.current = idty.login;
      toast.success(`Filled in ${idty.login}'s git identity`);
    } catch (e) {
      if (autofillSeq.current !== seq) return;
      setError(String(e));
      // Don't leave the dropdown pointing at an account we never loaded, with
      // the previous account's chips underneath it.
      setAutofillAccount(previous);
      setSuggestions({ names: [], emails: [] });
    } finally {
      if (autofillSeq.current === seq) setAutofilling(false);
    }
  }

  /**
   * Background: show which account a loaded profile belongs to.
   * Deliberately touches ONLY the dropdown and the suggestion chips — never the
   * identity fields, so a slow lookup can never clobber what's on screen.
   */
  async function preselectAccount(account: string, isCancelled: () => boolean) {
    const seq = ++autofillSeq.current;
    setAutofillAccount(account);
    try {
      const idty = await fetchIdentity(account);
      if (isCancelled() || userPicked.current || autofillSeq.current !== seq) return;
      setSuggestions({ names: idty.names, emails: idty.emails });
      setEmailHint(idty.scopeMissing ? scopeHint(idty.login) : null);
    } catch {
      // Leave the chips empty; the dropdown still shows the matched account.
    }
  }

  useEffect(() => {
    let cancelled = false;
    const isCancelled = () => cancelled;

    api.listSshKeys()
      .then((k) => !cancelled && setSshKeys(k))
      .catch(() => {});

    // The gh account list is fetched independently: it shells out to
    // `gh auth status` (network, no timeout), and the form must never wait on it.
    const accountsPromise = api.ghListAccounts().catch(() => [] as string[]);
    accountsPromise.then((a) => !cancelled && setGhAccounts(a));

    if (!isEdit) {
      const fromSignIn = searchParams.get("account");
      if (fromSignIn) void preselectAccount(fromSignIn, isCancelled);
      return () => {
        cancelled = true;
      };
    }

    (async () => {
      try {
        const profiles: Profile[] = await api.getProfiles();
        if (cancelled) return;
        const profile = profiles.find((p) => p.id === id);
        if (!profile) {
          setError("Profile not found — it may have been deleted.");
          return;
        }

        setName(profile.name);
        setGitName(profile.git_name);
        setGitEmail(profile.git_email);
        setSshKeyPath(profile.ssh_key_path ?? "");
        setAllowPush(profile.allow_push);
      setSigningEnabled(profile.signing_enabled);
        setIsDefaultProfile(profile.is_default);
        setDirectories(profile.directories);
        setRemotePatterns(profile.remote_patterns ?? []);
        setGithubLogin(profile.github_login ?? "");
        setOriginalIdentity({ name: profile.git_name, email: profile.git_email });

        // Work out which GitHub account this profile is, in the background.
        // First a provisional match on the git username (instant), then the
        // authoritative answer: which account the SSH key actually
        // authenticates as — that's what decides pushes.
        void (async () => {
          const accounts = await accountsPromise;
          if (cancelled || userPicked.current) return;
          setPreselecting(true);
          try {
            // The account the profile records comes first: it is what its
            // pushes are checked against, whatever its git username says.
            const byStored = accountValueFor(profile.github_login ?? "", accounts);
            if (byStored) {
              await preselectAccount(byStored, isCancelled);
              return;
            }
            const byName = accountValueFor(profile.git_name, accounts);
            if (byName) await preselectAccount(byName, isCancelled);
            if (!profile.ssh_key_path) return;
            const login = await api
              .resolveKeyAccount(profile.ssh_key_path)
              .catch(() => null);
            if (cancelled || userPicked.current || !login) return;
            const byKey = accountValueFor(login, accounts);
            if (byKey && byKey !== byName) await preselectAccount(byKey, isCancelled);
          } finally {
            if (!cancelled) setPreselecting(false);
          }
        })();
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoadingProfile(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [id, isEdit]);

  const accountOptions = [
    ...ghAccounts.map((a) => ({ value: a, label: a })),
    ...signins
      .filter((s) => !ghAccounts.some((a) => a.toLowerCase() === s.login.toLowerCase()))
      .map((s) => ({ value: `${SIGNIN_PREFIX}${s.login}`, label: `${s.login} · signed in with GitHub` })),
  ];
  /** The value in the picker for a login, from either source. */
  const accountValueFor = (login: string, gh: string[]): string | undefined => {
    const l = login.trim().replace(/^@/, "").toLowerCase();
    if (!l) return undefined;
    const fromGh = gh.find((a) => a.toLowerCase() === l);
    if (fromGh) return fromGh;
    const saved = signins.find((s) => s.login.toLowerCase() === l);
    return saved ? `${SIGNIN_PREFIX}${saved.login}` : undefined;
  };
  const pickedLogin = loginOfValue(autofillAccount);

  async function handleSubmit(e: SubmitEvent<HTMLFormElement>) {
    e.preventDefault();
    setLoading(true);
    setError(null);

    try {
      if (isEdit) {
        await api.updateProfile({
          id: id!,
          name,
          gitName,
          gitEmail,
          // "" = clear the key (backend treats empty string as "No SSH key")
          sshKeyPath: sshKeyPath,
          directories,
          allowPush,
          signingEnabled,
          remotePatterns,
          // Only a pick the person made changes the stored account; the
          // background preselection is a display guess, not a decision.
          githubLogin: userPicked.current && autofillAccount ? pickedLogin : githubLogin,
        });
        toast.success(`Profile "${name}" updated successfully`);
      } else {
        await api.createProfile({
          name,
          gitName,
          gitEmail,
          sshKeyPath: sshKeyPath,
          directories,
          allowPush,
          signingEnabled,
          remotePatterns,
          githubLogin: (userPicked.current && pickedLogin) || githubLogin || null,
        });
        toast.success(`Profile "${name}" created successfully`);
      }
      navigate("/");
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  function addDirectory() {
    const dir = dirInput.trim();
    if (!dir) return;
    // Functional update: never rebuild the list from a captured snapshot, or a
    // late add would wipe directories loaded after this closure was created.
    setDirectories((prev) => (prev.includes(dir) ? prev : [...prev, dir]));
    setDirInput("");
    toast.success("Folder added");
  }

  async function browseDirectory() {
    setError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select a folder for this profile",
      });
      if (typeof selected === "string") {
        // Functional update — the native picker can be open for a long time, so
        // a captured `directories` snapshot would be stale (and could wipe the
        // profile's saved folders).
        setDirectories((prev) =>
          prev.includes(selected) ? prev : [...prev, selected]
        );
        toast.success("Folder added");
      }
    } catch (e) {
      setError(String(e));
    }
  }

  function removeDirectory(dir: string) {
    setDirectories((prev) => prev.filter((d) => d !== dir));
  }

  // `acme` and `acme/` mean the same thing; the backend normalises the rest
  // (and refuses what it cannot read), so only the obvious is tidied here.
  function addRemotePattern() {
    const raw = remoteInput.trim();
    if (!raw) return;
    const p = /^[A-Za-z0-9_.-]+$/.test(raw) ? `${raw}/` : raw;
    setRemotePatterns((prev) => (prev.includes(p) ? prev : [...prev, p]));
    setRemoteInput("");
  }

  function removeRemotePattern(p: string) {
    setRemotePatterns((prev) => prev.filter((x) => x !== p));
  }

  async function registerSigning() {
    if (!sshKeyPath) return;
    setSigningBusy(true);
    setSigningMsg(null);
    setError(null);
    try {
      // Prefer the account the user picked; otherwise ask the key itself.
      // Registering goes through gh's token, so only a gh login can do it.
      let account = autofillAccount && !isSignInValue(autofillAccount) ? autofillAccount : "";
      if (!account) {
        const login = await api.resolveKeyAccount(sshKeyPath).catch(() => null);
        if (!login) {
          throw new Error(
            "Couldn't tell which GitHub account this key belongs to — pick one in \"Autofill from GitHub\" above."
          );
        }
        account = login;
      }
      const msg = await api.registerSigningKey(account, sshKeyPath);
      setSigningMsg(msg);
      toast.success(msg);
    } catch (e) {
      setSigningMsg(String(e));
    } finally {
      setSigningBusy(false);
    }
  }

  async function convertRepos() {
    setConverting(true);
    setConvertResults(null);
    setError(null);
    try {
      const all: api.RemoteChange[] = [];
      for (const dir of directories) {
        const res = await api.convertReposToSsh(dir);
        all.push(...res);
      }
      setConvertResults(all);
      const changed = all.filter((r) => r.changed).length;
      toast.success(
        changed > 0
          ? `Converted ${changed} repo${changed === 1 ? "" : "s"} to SSH`
          : "No repos needed converting"
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setConverting(false);
    }
  }

  const header = (
    <div className="flex items-center gap-3">
      <button
        onClick={() => navigate("/")}
        className="p-2 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer"
      >
        <ArrowLeft size={20} />
      </button>
      <h1 className="text-2xl font-bold text-zinc-100">
        {isEdit ? "Edit Profile" : "New Profile"}
      </h1>
    </div>
  );

  // Never render an interactive form before the saved profile is loaded —
  // typing into it would be discarded, and submitting it would clear the
  // profile's directories and SSH key.
  if (loadingProfile) {
    return (
      <div className="space-y-6">
        {header}
        <Card>
          <div className="flex items-center justify-center gap-2 py-10 text-sm text-zinc-400">
            <Loader2 size={16} className="animate-spin text-emerald-400" />
            Loading profile…
          </div>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {header}

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm">
          {error}
        </div>
      )}

      <form onSubmit={handleSubmit} className="space-y-6">
        {accountOptions.length > 0 && (
          <Card>
            <div className="flex items-center gap-2 mb-1">
              <GitHubIcon size={16} className="text-emerald-400" />
              <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider">
                Autofill from GitHub
              </h2>
            </div>
            <p className="text-xs text-zinc-500 mb-3">
              Pick a GitHub account (your <span className="font-mono">gh</span> logins, or an
              account signed in with GitHub on this computer) to fill in the git name &amp; email
              automatically.
            </p>
            <div className="flex items-center gap-2">
              <Select
                value={autofillAccount}
                onChange={(v) => applyAccountIdentity(v)}
                disabled={autofilling}
                placeholder="Select a GitHub account…"
                optionIcon={<GitHubIcon size={14} />}
                options={accountOptions}
                className="flex-1"
              />
              {(autofilling || preselecting) && (
                <Loader2 size={18} className="animate-spin text-emerald-400 shrink-0" aria-label="Looking up the account" />
              )}
            </div>
          </Card>
        )}

        {(githubLogin || (userPicked.current && autofillAccount)) && (
          <Card>
            <div className="flex items-center justify-between gap-3 flex-wrap">
              <p className="text-xs text-zinc-400">
                Pushes from this profile's repositories are expected to go as{" "}
                <span className="text-zinc-200">@{(userPicked.current && pickedLogin) || githubLogin}</span>.
                The Changes page says so when a push would go as someone else.
              </p>
              <Button
                type="button"
                variant="secondary"
                size="sm"
                onClick={() => {
                  setGithubLogin("");
                  setAutofillAccount("");
                  userPicked.current = true;
                }}
              >
                Isn't this account? Clear
              </Button>
            </div>
          </Card>
        )}

        <Card>
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-4">
            Basic Information
          </h2>
          <div className="space-y-4">
            <Input
              label="Profile Name"
              placeholder="e.g., Work, Personal, Open Source"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
            />
            <Input
              label="Git Username"
              placeholder="Your git user.name"
              value={gitName}
              onChange={(e) => setGitName(e.target.value)}
              required
            />
            <AltChips
              options={altOptions(suggestions.names, originalIdentity?.name, gitName)}
              onPick={setGitName}
            />
            <Input
              label="Git Email"
              placeholder="your@email.com"
              type="email"
              value={gitEmail}
              onChange={(e) => setGitEmail(e.target.value)}
              required
            />
            <AltChips
              options={altOptions(suggestions.emails, originalIdentity?.email, gitEmail)}
              onPick={setGitEmail}
            />
            {emailHint && <p className="text-xs text-amber-300/80 break-words">{emailHint}</p>}
          </div>
        </Card>

        <Card>
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-4">
            SSH Configuration
          </h2>
          <div className="space-y-3">
            <div className="space-y-1.5">
              <label className="block text-sm font-medium text-zinc-300">
                SSH Key
              </label>
              <Select
                value={sshKeyPath}
                onChange={setSshKeyPath}
                placeholder="No SSH key"
                optionIcon={<Key size={14} />}
                options={[
                  { value: "", label: "No SSH key" },
                  ...sshKeys.map((key) => ({
                    value: key,
                    label: baseName(key),
                  })),
                ]}
              />
            </div>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => navigate("/ssh")}
            >
              <Key size={14} />
              Generate New Key
            </Button>
          </div>
        </Card>

        <Card>
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-4">
            Directories
          </h2>
          <p className="text-xs text-zinc-500 mb-3">
            Assign directories where this profile should be used automatically.
            Git will use this identity for any repo inside these directories.
          </p>
          <Button
            type="button"
            variant="secondary"
            onClick={browseDirectory}
            className="mb-3"
          >
            <FolderOpen size={16} />
            Browse for folder…
          </Button>
          <div className="flex gap-2 mb-3">
            <Input
              placeholder="…or type a path, e.g. ~/projects/work"
              value={dirInput}
              onChange={(e) => setDirInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addDirectory();
                }
              }}
              containerClassName="flex-1 min-w-0"
            />
            <Button type="button" variant="secondary" onClick={addDirectory}>
              <FolderPlus size={16} />
              Add
            </Button>
          </div>
          {directories.length > 0 && (
            <div className="space-y-1.5">
              {directories.map((dir) => (
                <div
                  key={dir}
                  className="flex items-center justify-between px-3 py-2 rounded-lg bg-zinc-800/70 border border-zinc-700/50"
                >
                  <span className="text-sm text-zinc-300 font-mono truncate min-w-0">
                    {dir}
                  </span>
                  <button
                    type="button"
                    onClick={() => removeDirectory(dir)}
                    className="p-1 rounded hover:bg-zinc-700 text-zinc-500 hover:text-red-400 transition-colors cursor-pointer"
                  >
                    <X size={14} />
                  </button>
                </div>
              ))}
            </div>
          )}

          {directories.length > 0 && (
            <div className="mt-4 pt-4 border-t border-zinc-800">
              <div className="flex items-center justify-between gap-3 flex-wrap">
                <p className="text-xs text-zinc-500">
                  Make every repo in these folders push over SSH (removes any
                  embedded HTTPS tokens).
                </p>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={convertRepos}
                  disabled={converting}
                >
                  {converting ? (
                    <>
                      <Loader2 size={14} className="animate-spin" />
                      Converting…
                    </>
                  ) : (
                    <>
                      <Link2 size={14} />
                      Convert repos to SSH
                    </>
                  )}
                </Button>
              </div>
              {convertResults && (
                <div className="mt-3 space-y-1.5">
                  {convertResults.length === 0 ? (
                    <p className="text-xs text-zinc-500">
                      No git repositories found in these folders.
                    </p>
                  ) : (
                    convertResults.map((r) => (
                      <div
                        key={r.repo}
                        className="flex items-start gap-2 text-xs px-3 py-2 rounded-lg bg-zinc-800/50 border border-zinc-700/40"
                      >
                        {r.changed ? (
                          <CheckCircle2
                            size={14}
                            className="text-emerald-400 mt-0.5 shrink-0"
                          />
                        ) : (
                          <span className="w-3.5 h-3.5 mt-0.5 shrink-0" />
                        )}
                        <div className="min-w-0">
                          <p className="text-zinc-300 font-mono truncate">
                            {baseName(r.repo)}
                          </p>
                          <p className="text-zinc-500 font-mono break-all">
                            {r.changed ? r.new_url : r.note}
                          </p>
                        </div>
                      </div>
                    ))
                  )}
                </div>
              )}
            </div>
          )}
        </Card>

        <Card>
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-4 flex items-center gap-2">
            <Globe size={14} className="text-zinc-500" />
            Remotes
          </h2>
          <p className="text-xs text-zinc-500 mb-3">
            Claim GitHub owners or repositories: any clone whose remote is{" "}
            <span className="font-mono">github.com/owner/…</span> commits as this profile, wherever
            its folder is. A folder rule above still wins where both apply. Needs git 2.36 or newer.
            The Changes page knows such a clone's profile once it is opened there; Doctor's
            per-repository checks cover profile folders only.
          </p>
          <div className="flex gap-2 mb-3">
            <Input
              placeholder="owner/   or   owner/repo   or   https://gitlab.example.com/team/**"
              value={remoteInput}
              onChange={(e) => setRemoteInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addRemotePattern();
                }
              }}
              aria-label="Remote rule"
              containerClassName="flex-1 min-w-0"
            />
            <Button type="button" variant="secondary" onClick={addRemotePattern}>
              <Globe size={16} />
              Add
            </Button>
          </div>
          {remotePatterns.length > 0 && (
            <div className="space-y-1.5">
              {remotePatterns.map((p) => (
                <div
                  key={p}
                  className="flex items-center justify-between px-3 py-2 rounded-lg bg-zinc-800/70 border border-zinc-700/50"
                >
                  <span className="text-sm text-zinc-300 font-mono truncate min-w-0">{p}</span>
                  <button
                    type="button"
                    aria-label={`Remove remote rule ${p}`}
                    onClick={() => removeRemotePattern(p)}
                    className="p-1 rounded hover:bg-zinc-700 text-zinc-500 hover:text-red-400 transition-colors cursor-pointer"
                  >
                    <X size={14} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </Card>

        <Card>
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-3">
            Push Access
          </h2>
          <label
            className={`flex items-start gap-3 select-none ${
              isDefaultProfile ? "opacity-60 cursor-not-allowed" : "cursor-pointer"
            }`}
          >
            <Checkbox
              checked={allowPush}
              disabled={isDefaultProfile}
              onChange={setAllowPush}
              className="mt-0.5"
            />
            <span>
              <span className="text-sm text-zinc-200 font-medium">
                Allow pushing from this profile's folders
              </span>
              <span className="block text-xs text-zinc-500 mt-0.5">
                Uncheck to block <span className="font-mono">git push</span> locally
                for every repo in these folders — even if the account has push
                rights on GitHub. Pull and fetch keep working. Pushes fail with a
                "<span className="font-mono">gitswitch-push-blocked</span>" error.
              </span>
              {isDefaultProfile ? (
                <span className="block text-xs text-amber-400/80 mt-1">
                  The default profile can't block pushes — it would affect every
                  folder on the machine. Use a non-default profile instead.
                </span>
              ) : (
                <span className="block text-xs text-zinc-600 mt-1">
                  Covers every remote of every repository in this profile's
                  folders, whatever its address: GitHub in any form, an ssh-host
                  alias, another host, a local path. What gets past it: a remote
                  with its own push URL (remote.&lt;name&gt;.pushurl), a more
                  specific rewrite of your own, and a git that does not read your
                  global config (another HOME or GIT_CONFIG_GLOBAL). A folder profile that allows
                  pushes does not undo a parent profile's block. For a block that
                  needs the administrator password to remove, lock the repository
                  on the Changes page.
                </span>
              )}
            </span>
          </label>
        </Card>

        <Card>
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-3">
            Commit Signing
          </h2>
          <label
            className={`flex items-start gap-3 select-none ${
              sshKeyPath ? "cursor-pointer" : "opacity-60 cursor-not-allowed"
            }`}
          >
            <Checkbox
              checked={signingEnabled}
              disabled={!sshKeyPath}
              onChange={setSigningEnabled}
              className="mt-0.5"
            />
            <span className="min-w-0">
              <span className="text-sm text-zinc-200 font-medium">
                Sign commits and tags with this profile's SSH key
              </span>
              <span className="block text-xs text-zinc-500 mt-0.5">
                Sets <span className="font-mono">gpg.format=ssh</span> and{" "}
                <span className="font-mono">commit.gpgsign</span> for these folders, and
                keeps <span className="font-mono">~/.ssh/allowed_signers</span> in sync so
                signatures verify locally too.
              </span>
              {!sshKeyPath && (
                <span className="block text-xs text-amber-400/80 mt-1">
                  Pick an SSH key above first — signing uses that key's public half.
                </span>
              )}
            </span>
          </label>

          {signingEnabled && sshKeyPath && (
            <div className="mt-4 pt-4 border-t border-zinc-800">
              <p className="text-xs text-zinc-500 mb-2">
                For GitHub to show the <span className="text-zinc-300">Verified</span>{" "}
                badge, the same key must also be registered as a{" "}
                <strong className="text-zinc-300">signing</strong> key (that's a separate
                key type from an authentication key).
              </p>
              <div className="flex items-center gap-3 flex-wrap">
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={registerSigning}
                  disabled={signingBusy}
                >
                  {signingBusy ? (
                    <>
                      <Loader2 size={14} className="animate-spin" />
                      Registering…
                    </>
                  ) : (
                    <>
                      <GitHubIcon size={14} />
                      Register signing key on GitHub
                    </>
                  )}
                </Button>
                {signingMsg && (
                  <span className="text-xs text-zinc-400 break-words min-w-0">
                    {signingMsg}
                  </span>
                )}
              </div>
            </div>
          )}
        </Card>

        <div className="flex justify-end gap-3">
          <Button type="button" variant="secondary" onClick={() => navigate("/")}>
            Cancel
          </Button>
          <Button type="submit" disabled={loading}>
            {loading ? "Saving..." : isEdit ? "Update Profile" : "Create Profile"}
          </Button>
        </div>
      </form>
    </div>
  );
}

/** Values worth offering as one-click alternatives to what's in a field now. */
function altOptions(
  fromGitHub: string[],
  original: string | undefined,
  current: string
): string[] {
  return Array.from(new Set([...fromGitHub, original].filter(Boolean) as string[])).filter(
    (v) => v !== current
  );
}

function AltChips({
  options,
  onPick,
}: {
  options: string[];
  onPick: (value: string) => void;
}) {
  if (options.length === 0) return null;
  return (
    <div className="flex flex-wrap items-center gap-1.5 -mt-1">
      <span className="text-xs text-zinc-600">use instead:</span>
      {options.map((opt) => (
        <button
          key={opt}
          type="button"
          onClick={() => onPick(opt)}
          title={opt}
          className="px-2 py-0.5 rounded-md text-xs font-mono bg-zinc-800 border border-zinc-700 text-zinc-400 hover:text-emerald-300 hover:border-emerald-500/40 transition-colors cursor-pointer max-w-[22rem] truncate"
        >
          {opt}
        </button>
      ))}
    </div>
  );
}
