import { useState } from "react";
import {
  Key,
  Copy,
  CheckCircle2,
  XCircle,
  Loader2,
  ArrowRight,
} from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Input } from "../components/Input";
import * as api from "../lib/api";

type Step = "generate" | "pubkey" | "test";

export function SSHWizard() {
  const [step, setStep] = useState<Step>("generate");
  const [profileName, setProfileName] = useState("");
  const [email, setEmail] = useState("");
  const [privateKeyPath, setPrivateKeyPath] = useState("");
  const [publicKey, setPublicKey] = useState("");
  const [copied, setCopied] = useState(false);
  const [testResult, setTestResult] = useState<{
    status: "idle" | "testing" | "success" | "error";
    message: string;
  }>({ status: "idle", message: "" });
  const [error, setError] = useState<string | null>(null);
  const [generating, setGenerating] = useState(false);

  async function handleGenerate(e: React.FormEvent) {
    e.preventDefault();
    setGenerating(true);
    setError(null);
    try {
      const [privPath] = await api.generateSshKey(profileName, email);
      setPrivateKeyPath(privPath);
      const pubKey = await api.getPublicKey(privPath);
      setPublicKey(pubKey);
      setStep("pubkey");
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(false);
    }
  }

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(publicKey);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {}
  }

  async function handleTest() {
    setTestResult({ status: "testing", message: "Testing connection..." });
    try {
      const result = await api.testConnectionWithKey(privateKeyPath);
      setTestResult({ status: "success", message: result });
    } catch (e) {
      setTestResult({ status: "error", message: String(e) });
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-zinc-100">SSH Key Wizard</h1>
        <p className="text-sm text-zinc-400 mt-1">
          Generate a new SSH key for GitHub authentication
        </p>
      </div>

      <div className="flex items-center gap-2">
        {(["generate", "pubkey", "test"] as Step[]).map((s, i) => (
          <div key={s} className="flex items-center gap-2">
            <div
              className={`w-8 h-8 rounded-full flex items-center justify-center text-xs font-bold ${
                step === s
                  ? "bg-emerald-500 text-white"
                  : i < ["generate", "pubkey", "test"].indexOf(step)
                  ? "bg-emerald-500/20 text-emerald-400"
                  : "bg-zinc-800 text-zinc-500"
              }`}
            >
              {i + 1}
            </div>
            {i < 2 && (
              <div
                className={`w-12 h-0.5 ${
                  i < ["generate", "pubkey", "test"].indexOf(step)
                    ? "bg-emerald-500"
                    : "bg-zinc-700"
                }`}
              />
            )}
          </div>
        ))}
      </div>

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm">
          {error}
        </div>
      )}

      {step === "generate" && (
        <Card>
          <h2 className="text-lg font-semibold text-zinc-100 mb-4">
            Generate SSH Key
          </h2>
          <form onSubmit={handleGenerate} className="space-y-4">
            <Input
              label="Profile Name"
              placeholder="e.g., work, personal"
              value={profileName}
              onChange={(e) => setProfileName(e.target.value)}
              required
            />
            <Input
              label="Email"
              placeholder="your@email.com"
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              required
            />
            <p className="text-xs text-zinc-500">
              This will generate an Ed25519 SSH key pair at
              ~/.ssh/id_ed25519_{profileName || "<name>"}
            </p>
            <Button type="submit" disabled={generating}>
              {generating ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  Generating...
                </>
              ) : (
                <>
                  <Key size={16} />
                  Generate Key
                </>
              )}
            </Button>
          </form>
        </Card>
      )}

      {step === "pubkey" && (
        <Card>
          <h2 className="text-lg font-semibold text-zinc-100 mb-2">
            Your Public Key
          </h2>
          <p className="text-sm text-zinc-400 mb-4">
            Copy this key and add it to your GitHub account under Settings → SSH
            and GPG keys → New SSH key
          </p>
          <div className="relative">
            <pre className="p-4 bg-zinc-900 rounded-lg border border-zinc-700 text-xs text-zinc-300 font-mono overflow-x-auto whitespace-pre-wrap break-all">
              {publicKey}
            </pre>
            <button
              onClick={handleCopy}
              className="absolute top-2 right-2 p-2 rounded-lg bg-zinc-800 hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-colors"
            >
              {copied ? (
                <CheckCircle2 size={16} className="text-emerald-400" />
              ) : (
                <Copy size={16} />
              )}
            </button>
          </div>
          <p className="text-xs text-zinc-500 mt-2 font-mono">
            Key saved at: {privateKeyPath}
          </p>
          <div className="flex justify-end mt-4">
            <Button onClick={() => setStep("test")}>
              Next: Test Connection
              <ArrowRight size={16} />
            </Button>
          </div>
        </Card>
      )}

      {step === "test" && (
        <Card>
          <h2 className="text-lg font-semibold text-zinc-100 mb-4">
            Test Connection
          </h2>
          <p className="text-sm text-zinc-400 mb-4">
            Verify your SSH key works with GitHub. Make sure you've added the
            public key to your GitHub account first.
          </p>

          {testResult.status === "idle" && (
            <Button onClick={handleTest}>
              <Key size={16} />
              Test SSH Connection
            </Button>
          )}

          {testResult.status === "testing" && (
            <div className="flex items-center gap-2 text-zinc-400">
              <Loader2 size={16} className="animate-spin" />
              <span className="text-sm">Testing connection...</span>
            </div>
          )}

          {testResult.status === "success" && (
            <div className="space-y-3">
              <div className="flex items-center gap-2 text-emerald-400">
                <CheckCircle2 size={20} />
                <span className="font-medium">Connection successful!</span>
              </div>
              <pre className="p-3 bg-zinc-900 rounded-lg border border-zinc-700 text-xs text-zinc-400 font-mono">
                {testResult.message}
              </pre>
              <Button onClick={handleTest} variant="secondary" size="sm">
                Test Again
              </Button>
            </div>
          )}

          {testResult.status === "error" && (
            <div className="space-y-3">
              <div className="flex items-center gap-2 text-red-400">
                <XCircle size={20} />
                <span className="font-medium">Connection failed</span>
              </div>
              <pre className="p-3 bg-zinc-900 rounded-lg border border-zinc-700 text-xs text-red-400/80 font-mono whitespace-pre-wrap">
                {testResult.message}
              </pre>
              <p className="text-xs text-zinc-500">
                Make sure the public key has been added to your GitHub account.
              </p>
              <Button onClick={handleTest} variant="secondary" size="sm">
                Retry
              </Button>
            </div>
          )}
        </Card>
      )}
    </div>
  );
}
