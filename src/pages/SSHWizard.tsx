import { useState, useEffect, type SubmitEvent } from "react";
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
import { Select } from "../components/Select";
import { GitHubIcon } from "../components/GitHubIcon";
import { baseName } from "../lib/paths";
import { useRefreshOnFocus } from "../lib/focus";
import { copyText } from "../lib/clipboard";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "../lib/api";
import { SIGNIN_PREFIX } from "../lib/signins";
import { RefreshButton } from "../components/RefreshButton";

/**
 * Register one key on a GitHub account through a gh login, then say which
 * account the key authenticates as. Used for the key just generated and for
 * any key already in ~/.ssh — a key made on an earlier visit is no less
 * registrable.
 */
function RegisterKey({ keyPath, accounts }: { keyPath: string; accounts: string[] }) {
  const [account, setAccount] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const [resolved, setResolved] = useState<string | null>(null);
  const [verifying, setVerifying] = useState(false);

  async function verify() {
    setVerifying(true);
    setResolved(null);
    try {
      const acct = await api.resolveKeyAccount(keyPath);
      setResolved(acct ?? "(no GitHub account — not registered yet)");
    } catch (e) {
      setResolved(`error: ${String(e)}`);
    } finally {
      setVerifying(false);
    }
  }

  async function register() {
    if (!account) return;
    setBusy(true);
    setMessage(null);
    try {
      setMessage({ ok: true, text: await api.ghRegisterSshKey(account, keyPath) });
      void verify();
    } catch (e) {
      setMessage({ ok: false, text: String(e) });
    } finally {
      setBusy(false);
    }
  }

  // A key added by hand on github.com belongs to no app, so no organization's
  // third-party app restriction can hide its private repositories from it.
  async function addByHand() {
    setMessage(null);
    try {
      const copied = await copyText(await api.getPublicKey(keyPath));
      await openUrl("https://github.com/settings/ssh/new");
      setMessage(
        copied
          ? { ok: true, text: "The public key is copied. On GitHub, paste it into Key (any title), then click Add SSH key." }
          : { ok: false, text: "GitHub's page is open, but the key couldn't be copied: paste the contents of the .pub file by hand." }
      );
    } catch (e) {
      setMessage({ ok: false, text: String(e) });
    }
  }

  if (accounts.length === 0) {
    return (
      <p className="text-xs text-zinc-500">
        No GitHub account is signed in here — sign in on the GitHub page (with GitHub, or the
        GitHub CLI's <span className="font-mono">gh auth login</span>), then Refresh.
      </p>
    );
  }
  return (
    <div>
      <div className="flex gap-2">
        <Select
          value={account}
          onChange={setAccount}
          placeholder="Select an account…"
          optionIcon={<GitHubIcon size={14} />}
          options={accounts.map((a) => ({ value: a, label: a.startsWith(SIGNIN_PREFIX) ? `${a.slice(SIGNIN_PREFIX.length)} (signed in here)` : a }))}
          className="flex-1 min-w-0"
        />
        <Button type="button" onClick={register} disabled={!account || busy}>
          {busy ? (
            <>
              <Loader2 size={16} className="animate-spin" />
              Registering…
            </>
          ) : (
            "Register"
          )}
        </Button>
      </div>
      <p className="mt-2 text-xs text-zinc-500 leading-relaxed">
        GitHub marks a key that an app registers as added by that app, and an organization that restricts
        third-party apps (GitHub's default for new organizations) hides its private repositories from such keys.
        For those, add the key by hand instead; it works everywhere.{" "}
        <button type="button" onClick={() => void addByHand()} className="text-emerald-400 hover:underline cursor-pointer">
          Add it by hand on GitHub
        </button>
      </p>
      {message && (
        <div className={`mt-3 flex items-start gap-2 text-sm break-words ${message.ok ? "text-emerald-400" : "text-red-400"}`}>
          {message.ok ? <CheckCircle2 size={16} className="shrink-0 mt-0.5" /> : <XCircle size={16} className="shrink-0 mt-0.5" />}
          <span className="min-w-0">{message.text}</span>
        </div>
      )}
      {(verifying || resolved) && (
        <p className="mt-2 text-xs text-zinc-400 break-words">
          {verifying ? "Verifying which account this key maps to…" : "This key authenticates as: "}
          {!verifying && <span className="font-mono text-emerald-400">{resolved}</span>}
        </p>
      )}
    </div>
  );
}

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
  // Sign-ins kept here register keys too ("signin:<login>").
  const [keptSignins, setKeptSignins] = useState<string[]>([]);
  const loadKeptSignins = () =>
    api
      .githubSignins()
      .then((l) => setKeptSignins((Array.isArray(l) ? l : []).map((s) => SIGNIN_PREFIX + s.login)))
      .catch(() => setKeptSignins([]));
  const keyAccounts = [...ghAccounts, ...keptSignins];
  const [copyFailed, setCopyFailed] = useState(false);
  /** The existing key whose row is opened for registering. */
  const [openRow, setOpenRow] = useState<string | null>(null);
  const [rowNote, setRowNote] = useState<Record<string, string>>({});
  /** After a successful test: the account the key itself authenticates as. */
  const [testedAs, setTestedAs] = useState<string | null>(null);

  function loadKeys() {
    api.listSshKeys().then(setExistingKeys).catch(() => {});
  }
  const [reloading, setReloading] = useState(false);
  function reloadAll() {
    setReloading(true);
    Promise.all([
      api.listSshKeys().then(setExistingKeys).catch(() => {}),
      api.ghListAccounts().then(setGhAccounts).catch(() => setGhAccounts([])),
      loadKeptSignins(),
    ]).finally(() => setReloading(false));
  }

  // Keys are created and deleted in Terminal too.
  useRefreshOnFocus(() => loadKeys());

  useEffect(() => {
    loadKeys();
    api.ghListAccounts().then(setGhAccounts).catch(() => setGhAccounts([]));
    void loadKeptSignins();
  }, []);

  /** A row's public key to the clipboard, saying so when the webview refuses. */
  async function copyRowKey(k: string) {
    try {
      const ok = await copyText(await api.getPublicKey(k));
      setRowNote((n) => ({ ...n, [k]: ok ? "Public key copied." : "Couldn't copy — open the .pub file and copy it by hand." }));
    } catch (e) {
      setRowNote((n) => ({ ...n, [k]: String(e) }));
    }
  }

  async function checkRowAccount(k: string) {
    setRowNote((n) => ({ ...n, [k]: "Asking github.com which account this key is…" }));
    try {
      const acct = await api.resolveKeyAccount(k);
      setRowNote((n) => ({ ...n, [k]: acct ? `This key authenticates as: ${acct}` : "github.com refused this key: it isn't on any account yet." }));
    } catch (e) {
      setRowNote((n) => ({ ...n, [k]: String(e) }));
    }
  }

  async function handleGenerate(e: SubmitEvent<HTMLFormElement>) {
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
    if (await copyText(publicKey)) {
      setCopyFailed(false);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } else {
      setCopyFailed(true);
    }
  }

  async function handleTest() {
    setTestResult({ status: "testing", message: "Testing connection..." });
    setTestedAs(null);
    try {
      const result = await api.testConnectionWithKey(privateKeyPath);
      setTestResult({ status: "success", message: result });
      // The test runs through ~/.ssh/config, where another key can be pinned
      // for github.com: ask the key itself who it is.
      api.resolveKeyAccount(privateKeyPath).then((a) => setTestedAs(a ?? "")).catch(() => {});
    } catch (e) {
      setTestResult({ status: "error", message: String(e) });
    }
  }
  /** "Hi <login>!" in ssh's greeting. */
  const greeted = /Hi ([^!\s]+)!/.exec(testResult.message)?.[1] ?? null;

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">SSH Key Wizard</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Generate a new SSH key for GitHub authentication
          </p>
        </div>
        <RefreshButton onClick={reloadAll} busy={reloading} title="Re-read the keys on disk and the gh accounts" />
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
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm break-words">
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
            <p className="text-xs text-zinc-500 break-all">
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
          {copyFailed && (
            <p className="text-xs text-amber-300/80 mt-2">Couldn't copy — select the key above and copy it by hand.</p>
          )}
          <p className="text-xs text-zinc-500 mt-2 font-mono break-all">
            Key saved at: {privateKeyPath}
          </p>

          <div className="mt-5 pt-5 border-t border-zinc-800">
            <h3 className="text-sm font-semibold text-zinc-200 mb-1">
              Register this key on a GitHub account
            </h3>
            <p className="text-xs text-zinc-500 mb-3">
              Uses your local <span className="font-mono">gh</span> login — no
              copy-paste needed. A key can belong to only one account.
            </p>
            <RegisterKey keyPath={privateKeyPath} accounts={keyAccounts} />
          </div>

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
            <div className="flex gap-2 flex-wrap">
              <Button variant="secondary" onClick={() => setStep("pubkey")}>
                Back
              </Button>
              <Button onClick={handleTest}>
                <Key size={16} />
                Test SSH Connection
              </Button>
            </div>
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
              {greeted && <p className="text-sm text-zinc-300">Authenticated as <span className="font-mono text-emerald-400">{greeted}</span></p>}
              {testedAs !== null && greeted && testedAs.toLowerCase() !== greeted.toLowerCase() && (
                <p className="text-xs text-amber-300/80 break-words">
                  {testedAs
                    ? `On its own this key is ${testedAs}: the test went through another key that ~/.ssh/config gives github.com.`
                    : "On its own this key is refused by github.com: the test went through another key that ~/.ssh/config gives github.com. Register this one."}
                </p>
              )}
              <pre className="p-3 bg-zinc-900 rounded-lg border border-zinc-700 text-xs text-zinc-400 font-mono whitespace-pre-wrap break-words overflow-x-auto">
                {testResult.message}
              </pre>
              <div className="flex gap-2">
                <Button onClick={() => setStep("pubkey")} variant="secondary" size="sm">
                  Back
                </Button>
                <Button onClick={handleTest} variant="secondary" size="sm">
                  Test Again
                </Button>
              </div>
            </div>
          )}

          {testResult.status === "error" && (
            <div className="space-y-3">
              <div className="flex items-center gap-2 text-red-400">
                <XCircle size={20} />
                <span className="font-medium">Connection failed</span>
              </div>
              <pre className="p-3 bg-zinc-900 rounded-lg border border-zinc-700 text-xs text-red-400/80 font-mono whitespace-pre-wrap break-words overflow-x-auto">
                {testResult.message}
              </pre>
              <p className="text-xs text-zinc-500">
                Make sure the public key has been added to your GitHub account.
              </p>
              <div className="flex gap-2">
                <Button onClick={() => setStep("pubkey")} variant="secondary" size="sm">
                  Back
                </Button>
                <Button onClick={handleTest} variant="secondary" size="sm">
                  Retry
                </Button>
              </div>
            </div>
          )}
        </Card>
      )}

      <Card>
        <div className="flex items-center justify-between mb-4 gap-3 flex-wrap">
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
              <div key={k} className="rounded-lg bg-zinc-800/70 border border-zinc-700/50">
                <div className="flex items-center justify-between gap-3 px-3 py-2 flex-wrap">
                  <span className="text-sm text-zinc-300 font-mono truncate min-w-0 flex-1">
                    {baseName(k)}
                  </span>
                  <div className="flex items-center gap-1 shrink-0">
                    <Button size="sm" variant="ghost" onClick={() => setOpenRow(openRow === k ? null : k)} aria-expanded={openRow === k}>
                      Register
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => copyRowKey(k)}>
                      Copy public key
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => checkRowAccount(k)}>
                      Check account
                    </Button>
                    <button
                      onClick={() => setKeyToDelete(k)}
                      className="p-1.5 rounded shrink-0 hover:bg-zinc-700 text-zinc-500 hover:text-red-400 transition-colors cursor-pointer"
                      title="Delete key"
                    >
                      <Trash2 size={15} />
                    </button>
                  </div>
                </div>
                {rowNote[k] && <p className="px-3 pb-2 text-xs text-zinc-400 break-words">{rowNote[k]}</p>}
                {openRow === k && (
                  <div className="px-3 pb-3">
                    <RegisterKey keyPath={k} accounts={keyAccounts} />
                  </div>
                )}
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
          {keyToDelete ? baseName(keyToDelete) : ""}
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
