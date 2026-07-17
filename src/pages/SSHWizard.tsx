import { useState, useEffect } from "react";
import {
  Key,
  Copy,
  CheckCircle2,
  XCircle,
  Loader2,
  ArrowRight,
  Trash2,
} from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Input } from "../components/Input";
import { Modal } from "../components/Modal";
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
  const [existingKeys, setExistingKeys] = useState<string[]>([]);
  const [keyToDelete, setKeyToDelete] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [ghAccounts, setGhAccounts] = useState<string[]>([]);
  const [registerAccount, setRegisterAccount] = useState("");
  const [registering, setRegistering] = useState(false);
  const [registerMsg, setRegisterMsg] = useState<string | null>(null);
  const [resolvedAccount, setResolvedAccount] = useState<string | null>(null);
  const [verifying, setVerifying] = useState(false);

  function loadKeys() {
    api.listSshKeys().then(setExistingKeys).catch(() => {});
  }

  useEffect(() => {
    loadKeys();
    api.ghListAccounts().then(setGhAccounts).catch(() => setGhAccounts([]));
  }, []);

  async function handleRegister() {
    if (!registerAccount) return;
    setRegistering(true);
    setRegisterMsg(null);
    setError(null);
    try {
      const msg = await api.ghRegisterSshKey(
        registerAccount,
        privateKeyPath,
        "GitSwitch (macbook)"
      );
      setRegisterMsg(msg);
      // Confirm which account the key now maps to.
      handleVerify();
    } catch (e) {
      setError(String(e));
    } finally {
      setRegistering(false);
    }
  }

  async function handleVerify() {
    setVerifying(true);
    setResolvedAccount(null);
    try {
      const acct = await api.resolveKeyAccount(privateKeyPath);
      setResolvedAccount(acct ?? "(no GitHub account — not registered yet)");
    } catch (e) {
      setResolvedAccount(`error: ${String(e)}`);
    } finally {
      setVerifying(false);
    }
  }

  async function handleGenerate(e: React.FormEvent) {
    e.preventDefault();
    setGenerating(true);
    setError(null);
    try {
      const [privPath] = await api.generateSshKey(profileName, email);
      setPrivateKeyPath(privPath);
      const pubKey = await api.getPublicKey(privPath);
      setPublicKey(pubKey);
      loadKeys();
      setStep("pubkey");
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(false);
    }
  }

  async function handleDeleteKey() {
    if (!keyToDelete) return;
    setDeleting(true);
    setError(null);
    try {
      await api.deleteSshKey(keyToDelete);
      setKeyToDelete(null);
      loadKeys();
    } catch (e) {
      setError(String(e));
    } finally {
      setDeleting(false);
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
            <div className="flex items-center gap-2">
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
              <span
                className={`text-xs font-medium hidden sm:inline ${
                  step === s ? "text-emerald-400" : "text-zinc-500"
                }`}
              >
                {s === "generate" ? "Generate" : s === "pubkey" ? "Copy Key" : "Test"}
              </span>
            </div>
            {i < 2 && (
              <div
                className={`w-8 h-0.5 ${
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
              className="absolute top-2 right-2 p-2 rounded-lg bg-zinc-800 hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer"
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

          {ghAccounts.length > 0 && (
            <div className="mt-5 pt-5 border-t border-zinc-800">
              <h3 className="text-sm font-semibold text-zinc-200 mb-1">
                Register this key on a GitHub account
              </h3>
              <p className="text-xs text-zinc-500 mb-3">
                Uses your local <span className="font-mono">gh</span> login — no
                copy-paste needed. A key can belong to only one account.
              </p>
              <div className="flex gap-2">
                <select
                  value={registerAccount}
                  onChange={(e) => setRegisterAccount(e.target.value)}
                  className="flex-1 px-3 py-2 bg-zinc-800 border border-zinc-700 rounded-lg text-zinc-100 text-sm focus:outline-none focus:ring-2 focus:ring-emerald-500/50"
                >
                  <option value="">Select an account…</option>
                  {ghAccounts.map((a) => (
                    <option key={a} value={a}>
                      {a}
                    </option>
                  ))}
                </select>
                <Button
                  type="button"
                  onClick={handleRegister}
                  disabled={!registerAccount || registering}
                >
                  {registering ? (
                    <>
                      <Loader2 size={16} className="animate-spin" />
                      Registering…
                    </>
                  ) : (
                    "Register"
                  )}
                </Button>
              </div>
              {registerMsg && (
                <div className="mt-3 flex items-center gap-2 text-emerald-400 text-sm">
                  <CheckCircle2 size={16} />
                  {registerMsg}
                </div>
              )}
              {(verifying || resolvedAccount) && (
                <p className="mt-2 text-xs text-zinc-400">
                  {verifying
                    ? "Verifying which account this key maps to…"
                    : `This key authenticates as: `}
                  {!verifying && (
                    <span className="font-mono text-emerald-400">
                      {resolvedAccount}
                    </span>
                  )}
                </p>
              )}
            </div>
          )}

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
              <pre className="p-3 bg-zinc-900 rounded-lg border border-zinc-700 text-xs text-zinc-400 font-mono whitespace-pre-wrap break-words overflow-x-auto">
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

      <Card>
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-lg font-semibold text-zinc-100">
            Existing SSH Keys
          </h2>
          <span className="text-xs text-zinc-500">
            {existingKeys.length} key{existingKeys.length === 1 ? "" : "s"}
          </span>
        </div>
        {existingKeys.length === 0 ? (
          <p className="text-sm text-zinc-500">No SSH keys found in ~/.ssh</p>
        ) : (
          <div className="space-y-1.5">
            {existingKeys.map((k) => (
              <div
                key={k}
                className="flex items-center justify-between gap-3 px-3 py-2 rounded-lg bg-zinc-800/70 border border-zinc-700/50"
              >
                <span className="text-sm text-zinc-300 font-mono truncate">
                  {k.split("/").pop()}
                </span>
                <button
                  onClick={() => setKeyToDelete(k)}
                  className="p-1.5 rounded shrink-0 hover:bg-zinc-700 text-zinc-500 hover:text-red-400 transition-colors cursor-pointer"
                  title="Delete key"
                >
                  <Trash2 size={15} />
                </button>
              </div>
            ))}
          </div>
        )}
      </Card>

      <Modal
        open={keyToDelete !== null}
        onClose={() => setKeyToDelete(null)}
        title="Delete SSH Key?"
      >
        <p className="text-sm text-zinc-400 mb-2">
          This permanently deletes the private and public key files from
          <span className="text-zinc-300"> ~/.ssh</span>:
        </p>
        <p className="text-xs font-mono text-zinc-300 bg-zinc-800 rounded-lg px-3 py-2 mb-4 break-all">
          {keyToDelete?.split("/").pop()}
        </p>
        <p className="text-xs text-zinc-500 mb-4">
          If this key is in use by a profile, that profile will lose its SSH
          config. This cannot be undone.
        </p>
        <div className="flex justify-end gap-3">
          <Button
            variant="secondary"
            onClick={() => setKeyToDelete(null)}
            disabled={deleting}
          >
            Cancel
          </Button>
          <Button variant="danger" onClick={handleDeleteKey} disabled={deleting}>
            {deleting ? (
              <>
                <Loader2 size={16} className="animate-spin" />
                Deleting...
              </>
            ) : (
              <>
                <Trash2 size={16} />
                Delete Key
              </>
            )}
          </Button>
        </div>
      </Modal>
    </div>
  );
}
