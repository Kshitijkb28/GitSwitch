import { useState, useEffect } from "react";
import { RefreshCw, Info, Download, ExternalLink } from "lucide-react";
import { GitHubIcon } from "../components/GitHubIcon";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Input } from "../components/Input";
import { useToast } from "../components/Toast";
import { getCustomClientId, setOAuthClientId } from "../lib/oauth";
import * as api from "../lib/api";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export function Settings() {
  const toast = useToast();
  const [gitConfig, setGitConfig] = useState("");
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [clientId, setClientId] = useState("");

  useEffect(() => {
    loadConfig();
    setClientId(getCustomClientId() ?? "");
  }, []);

  function saveClientId() {
    setOAuthClientId(clientId);
    toast.success(
      clientId.trim()
        ? "GitHub OAuth Client ID saved"
        : "OAuth Client ID cleared"
    );
  }

  async function loadConfig() {
    try {
      const config = await api.getCurrentGitConfig();
      setGitConfig(config);
    } catch (e) {
      const msg = String(e);
      if (msg.includes("Tauri runtime not available")) {
        setGitConfig("No managed config yet. Create a profile and assign directories to generate config.");
      } else {
        setGitConfig(`Error loading config: ${e}`);
      }
    }
  }

  async function handleReapply() {
    setLoading(true);
    setMessage(null);
    try {
      await api.applyGitConfig();
      await loadConfig();
      setMessage("Git config applied successfully!");
      setTimeout(() => setMessage(null), 3000);
    } catch (e) {
      const msg = String(e);
      if (msg.includes("Tauri runtime not available")) {
        setMessage("This action requires the desktop app.");
      } else {
        setMessage(`Error: ${e}`);
      }
    } finally {
      setLoading(false);
    }
  }

  async function handleCheckUpdate() {
    setChecking(true);
    setUpdateStatus(null);
    try {
      const update = await check();
      if (update) {
        setUpdateStatus(`Update available: v${update.version}`);
        let downloaded = 0;
        let contentLength = 0;
        await update.downloadAndInstall((event) => {
          switch (event.event) {
            case "Started":
              contentLength = event.data.contentLength ?? 0;
              setUpdateStatus(`Downloading... 0%`);
              break;
            case "Progress":
              downloaded += event.data.chunkLength;
              const pct = contentLength
                ? Math.round((downloaded / contentLength) * 100)
                : 0;
              setUpdateStatus(`Downloading... ${pct}%`);
              break;
            case "Finished":
              setUpdateStatus("Update installed! Restarting...");
              break;
          }
        });
        await relaunch();
      } else {
        setUpdateStatus("You're on the latest version!");
        setTimeout(() => setUpdateStatus(null), 3000);
      }
    } catch (e) {
      const msg = String(e);
      if (msg.includes("__TAURI") || msg.includes("not a function") || msg.includes("undefined")) {
        setUpdateStatus("Update check requires the desktop app.");
      } else {
        setUpdateStatus(`Update check failed: ${e}`);
      }
    } finally {
      setChecking(false);
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-zinc-100">Settings</h1>
        <p className="text-sm text-zinc-400 mt-1">
          View and manage your git configuration
        </p>
      </div>

      {message && (
        <div
          className={`px-4 py-3 rounded-lg text-sm ${
            message.startsWith("Error")
              ? "bg-red-500/10 border border-red-500/30 text-red-400"
              : "bg-emerald-500/10 border border-emerald-500/30 text-emerald-400"
          }`}
        >
          {message}
        </div>
      )}

      <Card>
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider">
            Current Git Config (Managed Sections)
          </h2>
          <Button
            variant="secondary"
            size="sm"
            onClick={handleReapply}
            disabled={loading}
          >
            <RefreshCw size={14} className={loading ? "animate-spin" : ""} />
            Re-apply Config
          </Button>
        </div>
        <pre className="p-4 bg-zinc-900 rounded-lg border border-zinc-700 text-xs text-zinc-300 font-mono overflow-x-auto whitespace-pre-wrap max-h-96 overflow-y-auto">
          {gitConfig || "No managed config found"}
        </pre>
      </Card>

      <Card>
        <div className="flex items-start gap-3">
          <Info size={18} className="text-zinc-500 mt-0.5 shrink-0" />
          <div className="space-y-2 text-sm text-zinc-400">
            <p>
              <strong className="text-zinc-300">How it works:</strong> GitSwitch
              manages a section of your ~/.gitconfig file using{" "}
              <code className="px-1.5 py-0.5 bg-zinc-800 rounded text-xs">
                includeIf
              </code>{" "}
              directives.
            </p>
            <p>
              Each profile gets its own config file with user.name, user.email,
              and core.sshCommand settings. When you work in a directory assigned
              to a profile, git automatically uses that profile's identity.
            </p>
            <p>
              Backups are created before any modification to your system config.
            </p>
          </div>
        </div>
      </Card>

      <Card>
        <div className="flex items-center gap-2 mb-2">
          <GitHubIcon size={16} className="text-zinc-300" />
          <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider">
            GitHub Sign-in (OAuth device flow)
          </h2>
        </div>
        <p className="text-xs text-zinc-500 mb-3">
          The "Sign in with GitHub" button works out of the box with a built-in
          OAuth App. You only need this if you want to use your <strong>own</strong>{" "}
          OAuth App instead — leave it blank to use the default.
        </p>
        <ol className="text-xs text-zinc-400 list-decimal pl-5 space-y-1 mb-3">
          <li>
            Create a free OAuth App and <strong>enable "Device Flow"</strong>{" "}
            <a
              href="https://github.com/settings/developers"
              target="_blank"
              rel="noreferrer"
              className="text-emerald-400 inline-flex items-center gap-1 hover:underline"
            >
              github.com/settings/developers <ExternalLink size={11} />
            </a>
          </li>
          <li>Copy its <strong>Client ID</strong> (<span className="font-mono">Ov23li…</span>) and paste it below to override the default.</li>
        </ol>
        <div className="flex gap-2">
          <Input
            placeholder="Ov23li…  (your OAuth App Client ID)"
            value={clientId}
            onChange={(e) => setClientId(e.target.value)}
            className="flex-1 font-mono"
          />
          <Button variant="secondary" onClick={saveClientId}>
            Save
          </Button>
        </div>
      </Card>

      <Card>
        <h2 className="text-sm font-semibold text-zinc-300 uppercase tracking-wider mb-3">
          About
        </h2>
        <div className="space-y-3">
          <div className="space-y-1 text-sm text-zinc-400">
            <p>
              <span className="text-zinc-500">Version:</span>{" "}
              <span className="text-zinc-300">0.1.0</span>
            </p>
            <p>
              <span className="text-zinc-500">Built with:</span>{" "}
              <span className="text-zinc-300">Tauri v2 + React</span>
            </p>
          </div>
          <div className="flex items-center gap-3">
            <Button
              variant="secondary"
              size="sm"
              onClick={handleCheckUpdate}
              disabled={checking}
            >
              <Download size={14} className={checking ? "animate-bounce" : ""} />
              Check for Updates
            </Button>
            {updateStatus && (
              <span className="text-xs text-zinc-400">{updateStatus}</span>
            )}
          </div>
        </div>
      </Card>
    </div>
  );
}
