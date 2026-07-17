import { useState, useEffect } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft, FolderPlus, X, Key, FolderOpen, Link2, Loader2, CheckCircle2 } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "../components/Button";
import { Input } from "../components/Input";
import { Card } from "../components/Card";
import { useToast } from "../components/Toast";
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
  const [directories, setDirectories] = useState<string[]>([]);
  const [dirInput, setDirInput] = useState("");
  const [sshKeys, setSshKeys] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [converting, setConverting] = useState(false);
  const [convertResults, setConvertResults] = useState<api.RemoteChange[] | null>(null);

  useEffect(() => {
    api.listSshKeys().then(setSshKeys).catch(() => {});

    if (isEdit) {
      api.getProfiles().then((profiles) => {
        const profile = profiles.find((p) => p.id === id);
        if (profile) {
          setName(profile.name);
          setGitName(profile.git_name);
          setGitEmail(profile.git_email);
          setSshKeyPath(profile.ssh_key_path ?? "");
          setDirectories(profile.directories);
        }
      });
    }
  }, [id, isEdit]);

  async function handleSubmit(e: React.FormEvent) {
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
          sshKeyPath: sshKeyPath || null,
          directories,
        });
        toast.success(`Profile "${name}" updated successfully`);
      } else {
        await api.createProfile({
          name,
          gitName,
          gitEmail,
          sshKeyPath: sshKeyPath || null,
          directories,
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
    if (dir && !directories.includes(dir)) {
      setDirectories([...directories, dir]);
      setDirInput("");
      toast.success("Folder added");
    }
  }

  async function browseDirectory() {
    setError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select a folder for this profile",
      });
      if (typeof selected === "string" && !directories.includes(selected)) {
        setDirectories([...directories, selected]);
        toast.success("Folder added");
      }
    } catch (e) {
      setError(String(e));
    }
  }

  function removeDirectory(dir: string) {
    setDirectories(directories.filter((d) => d !== dir));
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

  return (
    <div className="space-y-6">
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

      {error && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm">
          {error}
        </div>
      )}

      <form onSubmit={handleSubmit} className="space-y-6">
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
            <Input
              label="Git Email"
              placeholder="your@email.com"
              type="email"
              value={gitEmail}
              onChange={(e) => setGitEmail(e.target.value)}
              required
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
              <select
                value={sshKeyPath}
                onChange={(e) => setSshKeyPath(e.target.value)}
                className="w-full px-3 py-2 bg-zinc-800 border border-zinc-700 rounded-lg text-zinc-100 focus:outline-none focus:ring-2 focus:ring-emerald-500/50 focus:border-emerald-500"
              >
                <option value="">No SSH key</option>
                {sshKeys.map((key) => (
                  <option key={key} value={key}>
                    {key}
                  </option>
                ))}
              </select>
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
                  <span className="text-sm text-zinc-300 font-mono truncate">
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
              <div className="flex items-center justify-between gap-3">
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
