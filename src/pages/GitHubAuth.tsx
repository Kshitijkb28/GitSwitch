import { useState, useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { Copy, CheckCircle2, Loader2, ExternalLink } from "lucide-react";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import * as api from "../lib/api";

type AuthStep = "idle" | "waiting" | "success" | "error";

export function GitHubAuth() {
  const navigate = useNavigate();
  const [step, setStep] = useState<AuthStep>("idle");
  const [deviceCode, setDeviceCode] = useState<api.DeviceCodeResponse | null>(null);
  const [user, setUser] = useState<api.GitHubUser | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const pollingRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    return () => {
      if (pollingRef.current) clearInterval(pollingRef.current);
    };
  }, []);

  async function startDeviceFlow() {
    try {
      setError(null);
      setStep("waiting");
      const code = await api.githubDeviceCode();
      setDeviceCode(code);
      startPolling(code);
    } catch (e) {
      setError(String(e));
      setStep("error");
    }
  }

  function startPolling(code: api.DeviceCodeResponse) {
    const interval = (code.interval || 5) * 1000;
    const deadline = Date.now() + code.expires_in * 1000;

    pollingRef.current = setInterval(async () => {
      if (Date.now() > deadline) {
        if (pollingRef.current) clearInterval(pollingRef.current);
        setError("Authorization timed out. Please try again.");
        setStep("error");
        return;
      }

      try {
        const token = await api.githubPollToken(code.device_code);
        if (pollingRef.current) clearInterval(pollingRef.current);

        const ghUser = await api.verifyGithubToken(token.access_token);
        setUser(ghUser);

        await api.storeGithubToken("__oauth_default__", token.access_token);
        setStep("success");
      } catch (e) {
        const msg = String(e);
        if (msg.includes("authorization_pending") || msg.includes("slow_down")) {
          return;
        }
        if (pollingRef.current) clearInterval(pollingRef.current);
        setError(msg);
        setStep("error");
      }
    }, interval);
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
                className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-colors"
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

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-zinc-100">Sign in with GitHub</h1>
      </div>

      <Card className="text-center py-8">
        <div className="text-red-400 mb-4">
          <p className="font-medium">Authentication failed</p>
          <p className="text-sm text-zinc-500 mt-1">{error}</p>
        </div>
        <Button onClick={() => { setStep("idle"); setError(null); }}>
          Try Again
        </Button>
      </Card>
    </div>
  );
}
