import { useState, useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { Copy, CheckCircle2, Loader2, ExternalLink, Terminal, Trash2, RefreshCw } from "lucide-react";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Modal } from "../components/Modal";
import { getEffectiveClientId } from "../lib/oauth";
import * as api from "../lib/api";

type AuthStep = "idle" | "waiting" | "success" | "error";

export function GitHubAuth() {
  const navigate = useNavigate();
  const [step, setStep] = useState<AuthStep>("idle");
  const [deviceCode, setDeviceCode] = useState<api.DeviceCodeResponse | null>(null);
  const [user, setUser] = useState<api.GitHubUser | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [ghAccounts, setGhAccounts] = useState<string[]>([]);
  const [ghBusy, setGhBusy] = useState<string | null>(null);
  const [removeTarget, setRemoveTarget] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
  const pollTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pollGenRef = useRef(0);

  const [ghRefreshing, setGhRefreshing] = useState(false);

  function loadGhAccounts() {
    api.ghListAccounts().then(setGhAccounts).catch(() => setGhAccounts([]));
  }

  async function refreshGhAccounts() {
    setGhRefreshing(true);
    try {
      const accts = await api.ghListAccounts();
      setGhAccounts(accts);
    } catch {
      setGhAccounts([]);
    } finally {
      setTimeout(() => setGhRefreshing(false), 400);
    }
  }

  useEffect(() => {
    // Best-effort: populate accounts from the local gh CLI if it's available.
    loadGhAccounts();
    return () => {
      if (pollTimeoutRef.current) clearTimeout(pollTimeoutRef.current);
    };
  }, []);

  async function handleRemoveAccount() {
    if (!removeTarget) return;
    setRemoving(true);
    setError(null);
    try {
      await api.ghLogout(removeTarget);
      setRemoveTarget(null);
      loadGhAccounts();
    } catch (e) {
      setError(String(e));
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
      await api.storeGithubToken(`__gh_${account}__`, token);
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
    const clientId = getEffectiveClientId();
    setError(null);
    setStep("waiting");
    setDeviceCode(null);

    // The very first network request from the app can cold-start-fail; retry a
    // couple times before surfacing an error so users don't see a spurious "Try again".
    let lastErr: unknown = null;
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        const code = await api.githubDeviceCode(clientId);
        setDeviceCode(code);
        startPolling(code, clientId);
        return;
      } catch (e) {
        lastErr = e;
        await new Promise((r) => setTimeout(r, 700));
      }
    }
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
    pollGenRef.current++;
    const myGen = pollGenRef.current;

    const schedule = () => {
      pollTimeoutRef.current = setTimeout(tick, delay);
    };

    const tick = async () => {
      if (pollGenRef.current !== myGen) return; // superseded/cancelled
      if (Date.now() > deadline) {
        setError("The code expired before you authorized. Click Sign in for a fresh code.");
        setStep("error");
        return;
      }
      try {
        const token = await api.githubPollToken(code.device_code, clientId);
        if (pollGenRef.current !== myGen) return;
        const ghUser = await api.verifyGithubToken(token.access_token);
        if (pollGenRef.current !== myGen) return;
        setUser(ghUser);
        await api.storeGithubToken("__oauth_default__", token.access_token);
        setStep("success");
        return; // done — stop the loop
      } catch (e) {
        if (pollGenRef.current !== myGen) return;
        const msg = String(e);
        if (msg.includes("slow_down")) {
          delay += 5000; // GitHub told us to back off — obey it
          schedule();
          return;
        }
        if (msg.includes("authorization_pending")) {
          schedule(); // keep waiting at the same interval
          return;
        }
        // terminal errors
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
    await navigator.clipboard.writeText(deviceCode.user_code);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  function openGitHub() {
    if (!deviceCode) return;
    window.open(deviceCode.verification_uri, "_blank");
  }

  function handleCreateProfile() {
    if (!user) return;
    navigate(`/profile/new?github_name=${encodeURIComponent(user.login)}&github_email=${encodeURIComponent(user.email || "")}&git_name=${encodeURIComponent(user.name || user.login)}`);
  }

  if (step === "idle") {
    return (
      <div className="space-y-6">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Sign in with GitHub</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Authenticate with your GitHub account to auto-configure your profile
          </p>
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

        <Card>
            <div className="flex items-center justify-between mb-1">
              <div className="flex items-center gap-2">
                <Terminal size={18} className="text-emerald-400" />
                <h3 className="text-lg font-medium text-zinc-200">
                  Use GitHub CLI
                </h3>
              </div>
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
            <p className="text-sm text-zinc-500 mb-4">
              You're already signed in to these accounts via <span className="font-mono text-zinc-400">gh</span>.
              Pick one to use instantly — no code needed.
            </p>
            {ghAccounts.length === 0 ? (
              <p className="text-sm text-zinc-500 px-3 py-4 rounded-lg bg-zinc-800/40 border border-zinc-700/40 text-center">
                No <span className="font-mono">gh</span> accounts found. Run{" "}
                <span className="font-mono text-zinc-400">gh auth login</span> in
                a terminal, then click <span className="text-zinc-300">Refresh</span>.
              </p>
            ) : (
            <div className="space-y-1.5">
              {ghAccounts.map((acct) => (
                <div
                  key={acct}
                  className="flex items-center justify-between gap-3 px-3 py-2 rounded-lg bg-zinc-800/70 border border-zinc-700/50"
                >
                  <span className="text-sm text-zinc-200 font-mono truncate">
                    {acct}
                  </span>
                  <div className="flex items-center gap-1 shrink-0">
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
                      onClick={() => setRemoveTarget(acct)}
                      title="Remove this account from the GitHub CLI"
                      className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-500 hover:text-red-400 transition-colors cursor-pointer"
                    >
                      <Trash2 size={15} />
                    </button>
                  </div>
                </div>
              ))}
            </div>
            )}
          </Card>

        <Modal
          open={removeTarget !== null}
          onClose={() => setRemoveTarget(null)}
          title="Remove GitHub Account?"
        >
          <p className="text-sm text-zinc-400 mb-2">
            This logs{" "}
            <span className="font-mono text-zinc-200">{removeTarget}</span> out
            of the GitHub CLI (<span className="font-mono">gh</span>) on this
            machine.
          </p>
          <p className="text-xs text-zinc-500 mb-4">
            Your SSH keys and GitSwitch profiles are not affected. You can sign
            back in anytime with <span className="font-mono">gh auth login</span>.
          </p>
          <div className="flex justify-end gap-3">
            <Button
              variant="secondary"
              onClick={() => setRemoveTarget(null)}
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
            <div>
              <h3 className="text-lg font-semibold text-zinc-100">{user.name || user.login}</h3>
              <p className="text-sm text-zinc-400">@{user.login}</p>
              {user.email && (
                <p className="text-sm text-zinc-500">{user.email}</p>
              )}
            </div>
            <CheckCircle2 size={24} className="ml-auto text-emerald-400" />
          </div>

          <div className="flex gap-3">
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
            {error && <p className="text-sm text-zinc-500 mt-1">{error}</p>}
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
