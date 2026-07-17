import { useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  Plus,
  Star,
  Folder,
  Key,
  GitBranch,
  Trash2,
  Pencil,
  RefreshCw,
} from "lucide-react";
import { GitHubIcon } from "../components/GitHubIcon";
import type { Profile } from "../types/profile";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { Badge } from "../components/Badge";
import { Modal } from "../components/Modal";
import { useProfiles } from "../hooks/useProfiles";
import * as api from "../lib/api";

export function Dashboard() {
  const navigate = useNavigate();
  const { profiles, loading, error, refresh } = useProfiles();
  const [deleteTarget, setDeleteTarget] = useState<Profile | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  async function handleRefresh() {
    setRefreshing(true);
    setActionError(null);
    try {
      await refresh();
    } finally {
      // brief spin so the click registers visually even on a fast reload
      setTimeout(() => setRefreshing(false), 400);
    }
  }

  async function handleSetDefault(id: string) {
    try {
      setActionError(null);
      await api.setDefaultProfile(id);
      await refresh();
    } catch (e) {
      setActionError(String(e));
    }
  }

  async function handleDelete() {
    if (!deleteTarget) return;
    try {
      setActionError(null);
      await api.deleteProfile(deleteTarget.id);
      setDeleteTarget(null);
      await refresh();
    } catch (e) {
      setActionError(String(e));
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-pulse text-zinc-500">Loading profiles...</div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-zinc-100">Profiles</h1>
          <p className="text-sm text-zinc-400 mt-1">
            Manage your GitHub identities
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="secondary"
            onClick={handleRefresh}
            title="Reload profiles from disk"
          >
            <RefreshCw
              size={16}
              className={refreshing ? "animate-spin" : ""}
            />
            Refresh
          </Button>
          <Button onClick={() => navigate("/profile/new")}>
            <Plus size={16} />
            Add Profile
          </Button>
        </div>
      </div>

      {(error || actionError) && (
        <div className="px-4 py-3 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-sm">
          {error || actionError}
        </div>
      )}

      {profiles.length === 0 ? (
        <Card className="text-center py-12">
          <GitBranch size={40} className="mx-auto text-zinc-600 mb-3" />
          <h3 className="text-lg font-medium text-zinc-300">
            No profiles yet
          </h3>
          <p className="text-sm text-zinc-500 mt-1 mb-4">
            Create your first GitHub profile to get started
          </p>
          <div className="flex items-center justify-center gap-3">
            <Button onClick={() => navigate("/profile/new")}>
              <Plus size={16} />
              Create Profile
            </Button>
            <Button variant="secondary" onClick={() => navigate("/github")}>
              <GitHubIcon size={16} />
              Sign in with GitHub
            </Button>
          </div>
        </Card>
      ) : (
        <div className="grid gap-3">
          {profiles.map((profile) => (
            <Card key={profile.id} active={profile.is_default}>
              <div className="flex items-start justify-between">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <h3 className="font-semibold text-zinc-100 truncate">
                      {profile.name}
                    </h3>
                    {profile.is_default && (
                      <Badge variant="success">Default</Badge>
                    )}
                  </div>
                  <p className="text-sm text-zinc-400 mt-0.5">
                    {profile.git_name} &lt;{profile.git_email}&gt;
                  </p>
                  <div className="flex items-center gap-4 mt-2">
                    <span className="inline-flex items-center gap-1 text-xs text-zinc-500">
                      <Key size={12} />
                      {profile.ssh_key_path ? (
                        <span className="text-emerald-400">SSH configured</span>
                      ) : (
                        <span className="text-zinc-500">No SSH key</span>
                      )}
                    </span>
                    <span className="inline-flex items-center gap-1 text-xs text-zinc-500">
                      <Folder size={12} />
                      {profile.directories.length} director
                      {profile.directories.length === 1 ? "y" : "ies"}
                    </span>
                  </div>
                </div>
                <div className="flex items-center gap-1 ml-3">
                  {!profile.is_default && (
                    <button
                      onClick={() => handleSetDefault(profile.id)}
                      title="Set as default"
                      className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-400 hover:text-amber-400 transition-colors cursor-pointer"
                    >
                      <Star size={16} />
                    </button>
                  )}
                  <button
                    onClick={() => navigate(`/profile/${profile.id}/edit`)}
                    title="Edit"
                    className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer"
                  >
                    <Pencil size={16} />
                  </button>
                  <button
                    onClick={() => setDeleteTarget(profile)}
                    title="Delete"
                    className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-400 hover:text-red-400 transition-colors cursor-pointer"
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
              </div>
            </Card>
          ))}
        </div>
      )}

      <Modal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title="Delete Profile"
      >
        <p className="text-sm text-zinc-400 mb-4">
          Are you sure you want to delete{" "}
          <strong className="text-zinc-200">{deleteTarget?.name}</strong>? This
          will remove the profile and its associated git configuration.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button variant="danger" onClick={handleDelete}>
            Delete
          </Button>
        </div>
      </Modal>
    </div>
  );
}
