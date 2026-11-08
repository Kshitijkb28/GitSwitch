import { useState, useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { Copy, CheckCircle2, Loader2, ExternalLink, Terminal, Trash2, RefreshCw } from "lucide-react";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Modal } from "../components/Modal";
import { Badge } from "../components/Badge";
import { useToast } from "../components/Toast";
import { getEffectiveClientId } from "../lib/oauth";
import { copyText } from "../lib/clipboard";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "../lib/api";
import { RefreshButton } from "../components/RefreshButton";
import { forgetSignIn, identityOf, rememberSignIn, rememberedSignIns, SIGNIN_PREFIX, type SignIn } from "../lib/signins";

type AuthStep = "idle" | "waiting" | "success" | "error";

/** GitHub's own answers that end a device-flow sign-in; anything else is tried again until the code expires. */
const FINAL_ERRORS = ["access_denied", "expired_token", "incorrect_device_code", "incorrect_client_credentials", "unsupported_grant_type", "device_flow_disabled"];

export function GitHubAuth() {
  const navigate = useNavigate();
  const [step, setStep] = useState<AuthStep>("idle");
  const [deviceCode, setDeviceCode] = useState<api.DeviceCodeResponse | null>(null);
  const [user, setUser] = useState<api.GitHubUser | null>(null);
  /** How the account on the Connected screen was reached: a gh login, or the device flow. */
  const [via, setVia] = useState<"gh" | "device">("device");
  const [error, setError] = useState<string | null>(null);
  /** A check GitHub did not answer, shown while the page keeps waiting. */
  const [retryNote, setRetryNote] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [ghAccounts, setGhAccounts] = useState<string[]>([]);
  const [ghBusy, setGhBusy] = useState<string | null>(null);
  const [removeTarget, setRemoveTarget] = useState<string | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const [signins, setSignins] = useState<SignIn[]>(rememberedSignIns);
  /** The sign-ins whose token the backend keeps (lower-case logins). */
  const [kept, setKept] = useState<Set<string>>(new Set());
  const loadKept = () =>
    api
      .githubSignins()
      .then((l) => setKept(new Set((Array.isArray(l) ? l : []).map((s) => s.login.toLowerCase()))))
      .catch(() => setKept(new Set()));
  /** Said on the Connected screen when the sign-in could not be kept. */
  const [keepNote, setKeepNote] = useState<string | null>(null);
  const [forgetError, setForgetError] = useState<string | null>(null);
  const pollTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pollGenRef = useRef(0);

  const [ghRefreshing, setGhRefreshing] = useState(false);
  const [ghLoading, setGhLoading] = useState(true);
  /** gh's listing with hosts and the active flag; empty when only the login list could be read. */
  const [ghRows, setGhRows] = useState<api.GhAccount[]>([]);
  /** "<host>/<login>" while gh switches to it. */
  const [ghSwitching, setGhSwitching] = useState<string | null>(null);
  const [ghSwitchError, setGhSwitchError] = useState<string | null>(null);
  /** `gh auth login` run by the app: asking gh for a code, then waiting with the code on screen. */
  const [ghLogin, setGhLogin] = useState<{ phase: "starting" } | { phase: "code"; prompt: api.GhDevicePrompt } | null>(null);
  /** What the last gh sign-in ended with, under the card. */
  const [ghLoginNote, setGhLoginNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [ghCodeCopied, setGhCodeCopied] = useState(false);
  /** Bumped by Cancel and by leaving the page: an answer for an older sign-in changes nothing. */
  const ghLoginGenRef = useRef(0);
  const ghLoginLiveRef = useRef(false);
  const toast = useToast();

  /** gh's own listing — host and active flag per login — when it answers; the bare login list otherwise. */
  async function readGhAccounts(): Promise<{ logins: string[]; rows: api.GhAccount[] }> {
    try {
      const rows = await api.ghAuthAccounts();
      const logins: string[] = [];
      for (const r of rows) if (!logins.includes(r.login)) logins.push(r.login);
      if (logins.length) return { logins, rows };
    } catch {
      /* the plain list below says what it can */
    }
    return { logins: await api.ghListAccounts(), rows: [] };
  }

  function loadGhAccounts() {
    setGhLoading(true);
    readGhAccounts()
      .then(({ logins, rows }) => {
        setGhAccounts(logins);
        setGhRows(rows);
      })
      .catch(() => {
        setGhAccounts([]);
        setGhRows([]);
      })
      .finally(() => setGhLoading(false));
  }

  async function refreshGhAccounts() {
    setGhRefreshing(true);
    setSignins(rememberedSignIns());
    void loadKept();
    try {
      const { logins, rows } = await readGhAccounts();
      setGhAccounts(logins);
      setGhRows(rows);
    } catch {
      setGhAccounts([]);
      setGhRows([]);
    } finally {
      setTimeout(() => setGhRefreshing(false), 400);
    }
  }

  /** `gh auth switch`: the account gh itself uses from now on. The listing is re-read afterwards. */
  async function makeGhActive(host: string, login: string) {
    setGhSwitching(`${host}/${login}`);
    setGhSwitchError(null);
    try {
      const said = await api.ghAuthSwitch(host, login);
      toast.success(said);
      const { logins, rows } = await readGhAccounts();
      setGhAccounts(logins);
      setGhRows(rows);
    } catch (e) {
      setGhSwitchError(String(e).replace(/^Command execution error: /, ""));
    } finally {
      setGhSwitching(null);
    }
  }

  useEffect(() => {
    // Best-effort: populate accounts from the local gh CLI if it's available.
    loadGhAccounts();
    void loadKept();
    return () => {
      // Leaving the page ends the sign-in: a check in flight, or a code
      // that arrives late, must start nothing more.
      pollGenRef.current++;
      if (pollTimeoutRef.current) clearTimeout(pollTimeoutRef.current);
      // A gh sign-in nobody can see the code of any more ends with the page.
      ghLoginGenRef.current++;
      if (ghLoginLiveRef.current) void api.ghAuthLoginCancel().catch(() => {});
    };
  }, []);

  /** `gh auth login --web` through the app: show gh's code, wait for it to be entered, then re-read gh's accounts. */
  async function startGhLogin() {
    const gen = ++ghLoginGenRef.current;
    const plain = (e: unknown) => String(e).replace(/^Command execution error: /, "");
    setGhLoginNote(null);
    setGhCodeCopied(false);
    setGhLogin({ phase: "starting" });
    ghLoginLiveRef.current = true;
    let prompt: api.GhDevicePrompt;
    try {
      prompt = await api.ghAuthLoginStart();
    } catch (e) {
      if (gen !== ghLoginGenRef.current) return;
      ghLoginLiveRef.current = false;
      setGhLogin(null);
      setGhLoginNote({ ok: false, text: plain(e) });
      return;
    }
    if (gen !== ghLoginGenRef.current) {
      // Cancelled while gh was still printing its code.
      void api.ghAuthLoginCancel().catch(() => {});
      return;
    }
    setGhLogin({ phase: "code", prompt });
    try {
      const who = await api.ghAuthLoginWait();
      if (gen !== ghLoginGenRef.current) return;
      setGhLoginNote({ ok: true, text: `Signed in to the GitHub CLI as ${who.login}. gh now uses it as its active account on ${who.host}.` });
      const { logins, rows } = await readGhAccounts().catch(() => ({ logins: [] as string[], rows: [] as api.GhAccount[] }));
      setGhAccounts(logins);
      setGhRows(rows);
    } catch (e) {
      if (gen !== ghLoginGenRef.current) return;
      setGhLoginNote({ ok: false, text: plain(e) });
    } finally {
      if (gen === ghLoginGenRef.current) {
        ghLoginLiveRef.current = false;
        setGhLogin(null);
      }
    }
  }

  function cancelGhLogin() {
    ghLoginGenRef.current++;
    ghLoginLiveRef.current = false;
    setGhLogin(null);
    void api.ghAuthLoginCancel().catch(() => {});
  }

  async function copyGhCode(code: string) {
    if (await copyText(code)) {
      setGhCodeCopied(true);
      setTimeout(() => setGhCodeCopied(false), 2000);
    }
  }

  function openRemove(acct: string) {
    setRemoveError(null);
    setRemoveTarget(acct);
  }

  function closeRemove() {
    if (removing) return;
    setRemoveTarget(null);
    setRemoveError(null);
  }

  async function handleRemoveAccount() {
    if (!removeTarget) return;
    setRemoving(true);
    setRemoveError(null);
    try {
      await api.ghLogout(removeTarget);
      setRemoveTarget(null);
      loadGhAccounts();
    } catch (e) {
      // Said where it was asked for, in the dialog; the list is read again,
      // so a row for an account already gone disappears.
      setRemoveError(String(e));
      loadGhAccounts();
    } finally {
      setRemoving(false);
    }
  }

  async function signInWithGh(account: string) {
    setError(null);
    setGhBusy(account);
    try {
      const token = await api.ghGetToken(account);
      const ghUser = await api.verifyGithubToken(token);
      setUser(ghUser);
      setVia("gh");
      // Note: tokens are not persisted here — gh already stores them securely,
      // and nothing in the app reads a per-account copy.
      setStep("success");
    } catch (e) {
      setError(String(e));
      setStep("error");
    } finally {
      setGhBusy(null);
    }
  }

  function stopPolling() {
    pollGenRef.current++; // invalidate any in-flight poll loop
    if (pollTimeoutRef.current) {
      clearTimeout(pollTimeoutRef.current);
      pollTimeoutRef.current = null;
    }
  }

  async function startDeviceFlow() {
    stopPolling(); // kill any poller from a previous attempt so it can't hijack the UI
    const myGen = pollGenRef.current;
    const clientId = getEffectiveClientId();
    setError(null);
    setRetryNote(null);
    setStep("waiting");
    setDeviceCode(null);

    // The very first network request from the app can cold-start-fail; retry a
    // couple times before surfacing an error so users don't see a spurious "Try again".
    // A code that arrives after Cancel, or after the page was left, is dropped.
    let lastErr: unknown = null;
    for (let attempt = 0; attempt < 3; attempt++) {
      if (pollGenRef.current !== myGen) return;
      try {
        const code = await api.githubDeviceCode(clientId);
        if (pollGenRef.current !== myGen) return;
        setDeviceCode(code);
        startPolling(code, clientId);
        return;
      } catch (e) {
        if (pollGenRef.current !== myGen) return;
        lastErr = e;
        await new Promise((r) => setTimeout(r, 700));
        if (pollGenRef.current !== myGen) return;
      }
    }
    if (pollGenRef.current !== myGen) return;
    setError(String(lastErr));
    setStep("error");
  }

  function startPolling(code: api.DeviceCodeResponse, clientId: string) {
    // GitHub's device flow REQUIRES honoring the interval and backing off on
    // "slow_down". A fixed setInterval that ignores slow_down gets throttled and
    // never receives the token — the classic "stuck on Waiting forever" bug.
    // So: recursive setTimeout, one request at a time, interval grows on slow_down.
    const deadline = Date.now() + code.expires_in * 1000;
    let delay = (code.interval || 5) * 1000;
    // Once GitHub has issued the token the device code is used up: a failure
    // after that retries reading the account with the same token, never polls again.
    let granted: string | null = null;
    let lastErr = "";
    pollGenRef.current++;
    const myGen = pollGenRef.current;

    const schedule = () => {
      pollTimeoutRef.current = setTimeout(tick, delay);
    };

    const tick = async () => {
      if (pollGenRef.current !== myGen) return; // superseded/cancelled
      if (Date.now() > deadline) {
        setError(
          granted
            ? `GitHub granted the sign-in, but the account could not be read: ${lastErr}`
            : "The code expired before you authorized. Click Sign in for a fresh code."
        );
        setStep("error");
        return;
      }
      try {
        if (!granted) {
          const token = await api.githubPollToken(code.device_code, clientId);
          if (pollGenRef.current !== myGen) return;
          granted = token.access_token;
        }
        const ghUser = await api.verifyGithubToken(granted);
        if (pollGenRef.current !== myGen) return;
        rememberSignIn(ghUser);
        setSignins(rememberedSignIns());
        // The token itself, in the system's credential store: with it this
        // account lists its repositories and registers keys like a gh login.
        try {
          await api.githubSigninKeep(granted);
          setKeepNote(null);
        } catch (e) {
          setKeepNote(`The sign-in could not be kept on this computer (${String(e)}): its name and email fill profiles, but listing its repositories and registering keys need the GitHub CLI.`);
        }
        void loadKept();
        setRetryNote(null);
        setUser(ghUser);
        setVia("device");
        setStep("success");
        return; // done — stop the loop
      } catch (e) {
        if (pollGenRef.current !== myGen) return;
        const msg = String(e);
        lastErr = msg;
        if (!granted && msg.includes("slow_down")) {
          // GitHub told us to back off: by its own interval when it sends one.
          const asked = Number(/interval=(\d+)/.exec(msg)?.[1]) || 0;
          delay = Math.max(delay + 5000, asked * 1000);
          setRetryNote(null);
          schedule();
          return;
        }
        if (!granted && msg.includes("authorization_pending")) {
          setRetryNote(null);
          schedule(); // keep waiting at the same interval
          return;
        }
        const final = !granted && (FINAL_ERRORS.some((f) => msg.includes(f)) || /GitHub returned 4(?!29)\d\d/.test(msg));
        if (!final) {
          // Not GitHub's answer — a check that timed out, a server error, a
          // reply that could not be read: the sign-in goes on until the code expires.
          setRetryNote(`GitHub did not answer the last check — still trying: ${msg}`);
          schedule();
          return;
        }
        if (msg.includes("access_denied")) {
          setError(
            "Authorization was denied or blocked. If GitHub showed an organization / third-party error, that account's org restricts OAuth apps — use \"Use GitHub CLI\" below instead."
          );
        } else if (msg.includes("expired_token")) {
          setError("The code expired. Click Sign in to get a fresh code.");
        } else {
          setError(msg);
        }
        setStep("error");
      }
    };

    schedule();
  }

  async function copyCode() {
    if (!deviceCode) return;
    // The webview's clipboard API can be unavailable; copyText has the fallback.
    if (await copyText(deviceCode.user_code)) {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  }

  function openGitHub() {
    if (!deviceCode) return;
    // window.open inside the webview opens nothing a person can use; the
    // opener plugin hands the address to the system browser.
    const uri = deviceCode.verification_uri;
    openUrl(uri).catch(() => {
      window.open(uri, "_blank");
    });
  }

  /** The same identity the profile form's Autofill gives: the login, and the public or no-reply email. */
  function handleCreateProfile() {
    if (!user) return;
    const idty = identityOf(user);
    const account = via === "device" && !ghAccounts.some((a) => a.toLowerCase() === user.login.toLowerCase()) ? `${SIGNIN_PREFIX}${user.login}` : user.login;
    const q = new URLSearchParams({ github_name: idty.login, github_email: idty.email, git_name: idty.login, account });
    navigate(`/profile/new?${q.toString()}`);
  }

  if (step === "idle") {
    return (
      <div className="space-y-6">
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div>
            <h1 className="text-2xl font-bold text-zinc-100">Sign in with GitHub</h1>
            <p className="text-sm text-zinc-400 mt-1">
              Authenticate with your GitHub account to auto-configure your profile
            </p>
          </div>
          <RefreshButton onClick={refreshGhAccounts} busy={ghRefreshing} title="Re-check the accounts gh is signed in to" />
        </div>

        <Card className="text-center py-12">
          <GitHubIcon size={48} className="mx-auto text-zinc-400 mb-4" />
          <h3 className="text-lg font-medium text-zinc-200 mb-2">
            GitHub Device Authorization
          </h3>
          <p className="text-sm text-zinc-500 max-w-sm mx-auto mb-6">
            We'll give you a code to enter on GitHub.com. No passwords are shared with this app.
          </p>
          <Button onClick={startDeviceFlow}>
            <GitHubIcon size={16} />
            Sign in with GitHub
          </Button>
        </Card>

        {signins.length > 0 && (
          <Card>
            <h3 className="text-sm font-medium text-zinc-200 mb-1">Signed in with GitHub on this computer</h3>
            <p className="text-xs text-zinc-500 mb-3">
              Their name and email fill a profile from "Autofill from GitHub", and a sign-in kept in this
              computer's keychain lists the account's repositories and registers SSH keys, like a GitHub
              CLI login. Forget removes both.
            </p>
            {forgetError && <p className="text-xs text-red-300/90 mb-2 break-words">{forgetError}</p>}
            <div className="space-y-1.5">
              {signins.map((s) => (
                <div key={s.login} className="flex items-center justify-between gap-3 px-3 py-2 rounded-lg bg-zinc-800/70 border border-zinc-700/50">
                  <span className="min-w-0">
                    <span className="block text-sm text-zinc-200 font-mono truncate">{s.login}</span>
                    <span className="block text-[11px] text-zinc-500">
                      {kept.has(s.login.toLowerCase()) ? "Sign-in kept in the keychain" : "Name and email only — sign in again to list its repositories here"}
                    </span>
                  </span>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={async () => {
                      setForgetError(null);
                      try {
                        await api.githubSigninForget(s.login);
                      } catch (e) {
                        setForgetError(`The keychain kept the sign-in for ${s.login}: ${String(e)}`);
                        return;
                      }
                      forgetSignIn(s.login);
                      setSignins(rememberedSignIns());
                      void loadKept();
                    }}
                  >
                    Forget
                  </Button>
                </div>
              ))}
            </div>
          </Card>
        )}

        <Card>
            <div className="flex items-center justify-between mb-1 gap-3 flex-wrap">
              <div className="flex items-center gap-2">
                <Terminal size={18} className="text-emerald-400" />
                <h3 className="text-lg font-medium text-zinc-200">
                  Use GitHub CLI
                </h3>
              </div>
              <div className="flex items-center gap-1">
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => void startGhLogin()}
                  disabled={ghLogin !== null}
                  title="gh auth login --web: gh's own sign-in in the browser, with SSH for git and no key uploaded"
                >
                  <GitHubIcon size={14} />
                  Sign in to GitHub CLI
                </Button>
                <button
                  onClick={refreshGhAccounts}
                  disabled={ghRefreshing}
                  title="Re-check gh accounts"
                  className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800 transition-colors cursor-pointer disabled:opacity-50"
                >
                  <RefreshCw size={14} className={ghRefreshing ? "animate-spin" : ""} />
                  Refresh
                </button>
              </div>
            </div>
            <p className="text-sm text-zinc-500 mb-4">
              You're already signed in to these accounts via <span className="font-mono text-zinc-400">gh</span>.
              Pick one to use instantly — no code needed.
            </p>
            {ghLogin && (
              <div className="mb-4 px-4 py-5 rounded-lg bg-zinc-900 border border-zinc-700 text-center" aria-live="polite">
                {ghLogin.phase === "starting" ? (
                  <p className="flex items-center justify-center gap-2 text-sm text-zinc-400">
                    <Loader2 size={16} className="animate-spin text-emerald-400" />
                    Asking <span className="font-mono">gh</span> for a sign-in code…
                  </p>
                ) : (
                  <>
                    <p className="text-xs text-zinc-500 uppercase tracking-wider mb-2">GitHub CLI code</p>
                    <div className="inline-flex items-center gap-3 bg-zinc-950 border border-zinc-700 rounded-xl px-5 py-3">
                      <span className="text-2xl font-mono font-bold tracking-[0.3em] text-emerald-400">{ghLogin.prompt.code}</span>
                      <button
                        onClick={() => void copyGhCode(ghLogin.prompt.code)}
                        title="Copy the code"
                        className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer"
                      >
                        {ghCodeCopied ? <CheckCircle2 size={18} className="text-emerald-400" /> : <Copy size={18} />}
                      </button>
                    </div>
                    <div className="mt-4 flex justify-center">
                      <Button
                        onClick={() => {
                          const uri = ghLogin.prompt.url;
                          openUrl(uri).catch(() => window.open(uri, "_blank"));
                        }}
                      >
                        <ExternalLink size={16} />
                        Open GitHub
                      </Button>
                    </div>
                    <p className="text-xs text-zinc-500 mt-2">
                      Enter the code at <span className="font-mono text-zinc-300">{ghLogin.prompt.url}</span>, signed in to GitHub as the account to add.
                    </p>
                    <p className="mt-4 flex items-center justify-center gap-2 text-sm text-zinc-500">
                      <Loader2 size={16} className="animate-spin" />
                      Waiting for GitHub…
                    </p>
                  </>
                )}
                <div className="flex justify-center mt-3">
                  <Button variant="ghost" size="sm" onClick={cancelGhLogin}>
                    Cancel
                  </Button>
                </div>
              </div>
            )}
            {ghLoginNote && (
              <p role={ghLoginNote.ok ? "status" : "alert"} className={`text-sm mb-4 break-words ${ghLoginNote.ok ? "text-emerald-300/90" : "text-red-400"}`}>
                {ghLoginNote.text}
              </p>
            )}
            {ghLoading ? (
              <div className="flex items-center justify-center gap-2 px-3 py-5 rounded-lg bg-zinc-800/40 border border-zinc-700/40 text-sm text-zinc-400">
                <Loader2 size={16} className="animate-spin text-emerald-400" />
                Checking GitHub CLI accounts…
              </div>
            ) : ghAccounts.length === 0 ? (
              <p className="text-sm text-zinc-500 px-3 py-4 rounded-lg bg-zinc-800/40 border border-zinc-700/40 text-center">
                No <span className="font-mono">gh</span> accounts found. Click{" "}
                <span className="text-zinc-300">Sign in to GitHub CLI</span>, or run{" "}
                <span className="font-mono text-zinc-400">gh auth login</span> in a terminal and click{" "}
                <span className="text-zinc-300">Refresh</span>.
              </p>
            ) : (
            <div className="space-y-1.5">
              {ghAccounts.map((acct) => {
                // The login's hosts as gh lists them; an older gh says nothing about what is active.
                const hosts = ghRows.filter((r) => r.login === acct);
                const activeOn = hosts.filter((r) => r.active === true).map((r) => r.host);
                const inactiveOn = hosts.filter((r) => r.active === false);
                const otherHosts = hosts.map((r) => r.host).filter((h) => h !== "github.com");
                return (
                <div
                  key={acct}
                  className="flex items-center justify-between gap-3 px-3 py-2 rounded-lg bg-zinc-800/70 border border-zinc-700/50"
                >
                  <span className="text-sm text-zinc-200 font-mono truncate min-w-0" title={hosts.map((r) => r.host).join(", ")}>
                    {acct}
                    {otherHosts.length > 0 && <span className="text-zinc-500"> · {otherHosts.join(", ")}</span>}
                  </span>
                  <div className="flex items-center gap-1 shrink-0">
                    {activeOn.length > 0 && (
                      <span title={`gh and git's HTTPS credential helper use this account on ${activeOn.join(", ")}`}>
                        <Badge variant="success">Active</Badge>
                      </span>
                    )}
                    {inactiveOn.map((r) => (
                      <Button
                        key={r.host}
                        size="sm"
                        variant="ghost"
                        disabled={ghSwitching !== null || ghBusy !== null}
                        onClick={() => void makeGhActive(r.host, acct)}
                        title={`gh auth switch --hostname ${r.host} --user ${acct} — the account gh and git's HTTPS credential helper use from now on; GitSwitch itself always names the account it uses`}
                      >
                        {ghSwitching === `${r.host}/${acct}` ? (
                          <>
                            <Loader2 size={14} className="animate-spin" />
                            Switching…
                          </>
                        ) : hosts.length > 1 ? (
                          `Make active on ${r.host}`
                        ) : (
                          "Make active"
                        )}
                      </Button>
                    ))}
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => signInWithGh(acct)}
                      disabled={ghBusy !== null}
                    >
                      {ghBusy === acct ? (
                        <>
                          <Loader2 size={14} className="animate-spin" />
                          Signing in...
                        </>
                      ) : (
                        "Use this account"
                      )}
                    </Button>
                    <button
                      onClick={() => openRemove(acct)}
                      title="Remove this account from the GitHub CLI"
                      className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-500 hover:text-red-400 transition-colors cursor-pointer"
                    >
                      <Trash2 size={15} />
                    </button>
                  </div>
                </div>
                );
              })}
            </div>
            )}
            {ghSwitchError && (
              <p className="text-sm text-red-400 mt-3 break-words" role="alert">
                {ghSwitchError}
              </p>
            )}
            {ghRows.some((r) => r.active !== null) && (
              <p className="text-xs text-zinc-500 mt-3">
                The active account is what <span className="font-mono">gh</span> and git's HTTPS credential helper use on their own;
                GitSwitch always says which account it is using.
              </p>
            )}
          </Card>

        <Modal
          open={removeTarget !== null}
          onClose={closeRemove}
          title="Remove GitHub Account?"
        >
          <p className="text-sm text-zinc-400 mb-2">
            This logs{" "}
            <span className="font-mono text-zinc-200">{removeTarget}</span> out
            of the GitHub CLI (<span className="font-mono">gh</span>) on this
            machine — from every host gh has it signed in to.
          </p>
          <p className="text-xs text-zinc-500 mb-4">
            Your SSH keys and GitSwitch profiles are not affected. You can sign
            back in anytime with <span className="font-mono">gh auth login</span>.
          </p>
          {removeError && (
            <p role="alert" className="text-sm text-red-400 mb-4 whitespace-pre-wrap break-words">
              {removeError}
            </p>
          )}
          <div className="flex justify-end gap-3">
            <Button
              variant="secondary"
              onClick={closeRemove}
              disabled={removing}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={handleRemoveAccount}
              disabled={removing}
            >
              {removing ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  Removing...
                </>
              ) : (
                <>
                  <Trash2 size={16} />
                  Remove Account
                </>
              )}
            </Button>
          </div>
        </Modal>
      </div>
    );
  }

  if (step === "waiting" && !deviceCode) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Sign in with GitHub</h1>
        </div>
        <Card className="text-center py-12">
          <Loader2 size={22} className="animate-spin mx-auto text-emerald-400 mb-3" />
          <p className="text-sm text-zinc-400">
            Requesting a sign-in code from GitHub…
          </p>
          <div className="flex justify-center mt-4">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                stopPolling();
                setStep("idle");
              }}
            >
              Cancel
            </Button>
          </div>
        </Card>
      </div>
    );
  }

  if (step === "waiting" && deviceCode) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Enter Code on GitHub</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Copy this code and enter it on GitHub to authorize GitSwitch
          </p>
        </div>

        <Card className="text-center py-8">
          <div className="mb-6">
            <p className="text-xs text-zinc-500 uppercase tracking-wider mb-2">Your code</p>
            <div className="inline-flex items-center gap-3 bg-zinc-900 border border-zinc-700 rounded-xl px-6 py-4">
              <span className="text-3xl font-mono font-bold tracking-[0.3em] text-emerald-400">
                {deviceCode.user_code}
              </span>
              <button
                onClick={copyCode}
                className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer"
              >
                {copied ? <CheckCircle2 size={20} className="text-emerald-400" /> : <Copy size={20} />}
              </button>
            </div>
          </div>

          <div className="space-y-3">
            <Button onClick={openGitHub}>
              <ExternalLink size={16} />
              Open GitHub
            </Button>
            <p className="text-xs text-zinc-500">
              Go to <span className="text-zinc-300 font-mono">{deviceCode.verification_uri}</span> and paste the code
            </p>
          </div>

          <div className="mt-8 flex items-center justify-center gap-2 text-sm text-zinc-500">
            <Loader2 size={16} className="animate-spin" />
            Waiting for authorization...
          </div>
          {retryNote && (
            <p className="text-xs text-amber-300/80 text-center mt-2 max-w-md mx-auto break-words">{retryNote}</p>
          )}
          <p className="text-xs text-zinc-600 text-center mt-3 max-w-sm mx-auto">
            This completes automatically once you authorize on GitHub. If GitHub
            shows a third-party / organization error there, that account blocks
            OAuth apps — cancel and use <span className="text-zinc-400">"Use GitHub CLI"</span> instead.
          </p>
          <div className="flex justify-center mt-4">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                stopPolling();
                setStep("idle");
              }}
            >
              Cancel
            </Button>
          </div>
        </Card>
      </div>
    );
  }

  if (step === "success" && user) {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Connected!</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Your GitHub account has been authenticated
          </p>
        </div>

        <Card className="py-8">
          <div className="flex items-center gap-4 mb-6">
            <img
              src={user.avatar_url}
              alt={user.login}
              className="w-16 h-16 rounded-full border-2 border-emerald-500"
            />
            <div className="min-w-0">
              <h3 className="text-lg font-semibold text-zinc-100 truncate">{user.name || user.login}</h3>
              <p className="text-sm text-zinc-400">@{user.login}</p>
              {user.email && (
                <p className="text-sm text-zinc-500 break-all">{user.email}</p>
              )}
            </div>
            <CheckCircle2 size={24} className="ml-auto text-emerald-400 shrink-0" />
          </div>
          {via === "device" && (
            <p className="text-xs text-zinc-500 mb-5">
              GitSwitch remembers this account's name and email for "Autofill from GitHub" when you
              create or edit a profile, and keeps the sign-in in this computer's keychain: the
              account lists its repositories on the Repositories page and registers SSH keys, like a
              GitHub CLI login. Forget on this page removes both.
            </p>
          )}
          {via === "device" && keepNote && <p className="text-xs text-amber-300/80 mb-5 break-words">{keepNote}</p>}

          <div className="flex gap-3 flex-wrap">
            <Button onClick={handleCreateProfile}>
              Create Profile from GitHub
            </Button>
            <Button variant="secondary" onClick={() => navigate("/")}>
              Back to Dashboard
            </Button>
          </div>
        </Card>
      </div>
    );
  }

  if (step === "error") {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Sign in with GitHub</h1>
        </div>

        <Card className="text-center py-8">
          <div className="text-red-400 mb-4">
            <p className="font-medium">Authentication failed</p>
            {error && <p className="text-sm text-zinc-500 mt-1 break-words">{error}</p>}
          </div>
          <Button onClick={() => { stopPolling(); setStep("idle"); setError(null); }}>
            Try Again
          </Button>
        </Card>
      </div>
    );
  }

  // Fallback (e.g. step === "waiting" before the code arrives): render nothing
  // instead of the error screen. The dedicated waiting branch below handles it.
  return null;
}
