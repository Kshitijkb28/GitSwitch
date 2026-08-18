import { useState, useEffect, useRef, type SubmitEvent } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft, FolderPlus, X, Key, FolderOpen, Link2, Loader2, CheckCircle2 } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { Card } from "../components/Card";
import { Select } from "../components/Select";
import { useToast } from "../components/Toast";
import type { Profile } from "../types/profile";
import * as api from "../lib/api";

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
  const [sshKeys, setSshKeys] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [converting, setConverting] = useState(false);
  const [convertResults, setConvertResults] = useState<api.RemoteChange[] | null>(null);
  const [ghAccounts, setGhAccounts] = useState<string[]>([]);
  const [autofillAccount, setAutofillAccount] = useState("");
  const [autofilling, setAutofilling] = useState(false);
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

  async function fetchIdentity(account: string) {
    const token = await api.ghGetToken(account);
    const u = await api.verifyGithubToken(token);
    const noreply = `${u.id}+${u.login}@users.noreply.github.com`;
    return {
      login: u.login,
      email: u.email || noreply,
      names: Array.from(new Set([u.login, u.name].filter(Boolean) as string[])),
      emails: Array.from(new Set([u.email, noreply].filter(Boolean) as string[])),
    };
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
      setGitEmail(idty.email);
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
        setOriginalIdentity({ name: profile.git_name, email: profile.git_email });

        // Work out which GitHub account this profile is, in the background.
        // First a provisional match on the git username (instant), then the
        // authoritative answer: which account the SSH key actually
        // authenticates as — that's what decides pushes.
        void (async () => {
          const accounts = await accountsPromise;
          if (cancelled || userPicked.current) return;
          const byName = accounts.find(
            (a) => a.toLowerCase() === profile.git_name.trim().toLowerCase()
          );
          if (byName) await preselectAccount(byName, isCancelled);
          if (!profile.ssh_key_path) return;
          const login = await api
            .resolveKeyAccount(profile.ssh_key_path)
            .catch(() => null);
          if (cancelled || userPicked.current || !login) return;
          const byKey = accounts.find((a) => a.toLowerCase() === login.toLowerCase());
          if (byKey && byKey !== byName) await preselectAccount(byKey, isCancelled);
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

  async function registerSigning() {
    if (!sshKeyPath) return;
    setSigningBusy(true);
    setSigningMsg(null);
    setError(null);
    try {
      // Prefer the account the user picked; otherwise ask the key itself.
      let account = autofillAccount;
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
        {ghAccounts.length > 0 && (
          <Card>
            <div className="flex items-center gap-2 mb-1">
              <GitHubIcon size={16} className="text-emerald-400" />
              <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider">
                Autofill from GitHub
              </h2>
            </div>
            <p className="text-xs text-zinc-500 mb-3">
              Pick a GitHub account (from your <span className="font-mono">gh</span>{" "}
              logins) to fill in the git name &amp; email automatically.
            </p>
            <div className="flex items-center gap-2">
              <Select
                value={autofillAccount}
                onChange={(v) => applyAccountIdentity(v)}
                disabled={autofilling}
                placeholder="Select a GitHub account…"
                optionIcon={<GitHubIcon size={14} />}
                options={ghAccounts.map((a) => ({ value: a, label: a }))}
                className="flex-1"
              />
              {autofilling && (
                <Loader2 size={18} className="animate-spin text-emerald-400 shrink-0" />
              )}
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
                    label: key.split("/").pop() || key,
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
              className="flex-1"
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
                            {r.repo.split("/").pop()}
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
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-3">
            Push Access
          </h2>
          <label
            className={`flex items-start gap-3 select-none ${
              isDefaultProfile ? "opacity-60 cursor-not-allowed" : "cursor-pointer"
            }`}
          >
            <input
              type="checkbox"
              checked={allowPush}
              disabled={isDefaultProfile}
              onChange={(e) => setAllowPush(e.target.checked)}
              className="accent-emerald-500 mt-1 cursor-pointer disabled:cursor-not-allowed"
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
                  Covers standard github.com remotes. Repos with a custom
                  ssh-host alias or an explicit push URL aren't blocked; if
                  folders overlap, a block from any covering profile wins.
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
            <input
              type="checkbox"
              checked={signingEnabled}
              disabled={!sshKeyPath}
              onChange={(e) => setSigningEnabled(e.target.checked)}
              className="accent-emerald-500 mt-1 cursor-pointer disabled:cursor-not-allowed"
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
