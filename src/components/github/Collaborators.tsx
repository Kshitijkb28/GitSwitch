import { useEffect, useRef, useState } from "react";
import { Loader2, Mail, UserMinus, UserPlus, Users, X } from "lucide-react";
import * as api from "../../lib/api";
import { Badge } from "../Badge";
import { Button } from "../Button";
import { Input } from "../Input";
import { Modal } from "../Modal";
import { RefreshButton } from "../RefreshButton";
import { Select } from "../Select";
import { useToast } from "../Toast";
import { ago, cleanError } from "./shared";

type Role = "pull" | "triage" | "push" | "maintain" | "admin";
const ROLES: { value: Role; label: string }[] = [
  { value: "pull", label: "Read" },
  { value: "triage", label: "Triage" },
  { value: "push", label: "Write" },
  { value: "maintain", label: "Maintain" },
  { value: "admin", label: "Admin" },
];

/**
 * Who a repository is shared with: its collaborators and the invitations
 * still waiting, inviting someone by username with a role, removing access.
 * Changing who has access needs an admin; anyone else sees why it is greyed.
 */
export function Collaborators({ account, owner, name, admin }: { account: string; owner: string; name: string; admin: boolean }) {
  const toast = useToast();
  const [people, setPeople] = useState<api.People | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [username, setUsername] = useState("");
  const [role, setRole] = useState<Role>("push");
  const [busy, setBusy] = useState(false);
  const [removing, setRemoving] = useState<api.Collaborator | null>(null);
  const ask = useRef(0);
  const orgish = people ? !people.collaborators.some((c) => c.is_owner) : true;

  async function load() {
    const id = ++ask.current;
    setLoading(true);
    setError(null);
    try {
      const p = await api.githubCollaborators(account, owner, name);
      if (id === ask.current) setPeople(p);
    } catch (e) {
      if (id === ask.current) {
        setPeople(null);
        setError(cleanError(e));
      }
    } finally {
      if (id === ask.current) setLoading(false);
    }
  }

  useEffect(() => {
    if (account) void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account, owner, name]);

  async function invite() {
    setBusy(true);
    setError(null);
    try {
      const out = await api.githubCollaboratorAdd(account, owner, name, username.trim(), role);
      toast.success(out.message);
      setUsername("");
      void load();
    } catch (e) {
      setError(cleanError(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove(c: api.Collaborator) {
    setBusy(true);
    setError(null);
    try {
      await api.githubCollaboratorRemove(account, owner, name, c.login);
      toast.success(`${c.login} no longer has access to ${owner}/${name}`);
      setRemoving(null);
      void load();
    } catch (e) {
      setRemoving(null);
      setError(cleanError(e));
    } finally {
      setBusy(false);
    }
  }

  async function cancel(inv: api.Invitation) {
    setBusy(true);
    setError(null);
    try {
      await api.githubInvitationCancel(account, owner, name, inv.id);
      toast.success(`Invitation for ${inv.login} cancelled`);
      void load();
    } catch (e) {
      setError(cleanError(e));
    } finally {
      setBusy(false);
    }
  }

  const noAdmin = admin ? null : "Only an admin of this repository can change who has access. Act as an account that is one.";

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2 flex-wrap">
        <h3 className="text-sm font-medium text-zinc-200 flex items-center gap-2">
          <Users size={15} className="text-zinc-400" />
          Collaborators{people ? ` · ${people.collaborators.length}` : ""}
        </h3>
        <RefreshButton onClick={() => void load()} busy={loading} title="Read the collaborators again" />
      </div>

      <div className="flex items-end gap-2 flex-wrap" aria-label="Invite a collaborator">
        <Input
          containerClassName="flex-1 min-w-[12rem]"
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && username.trim() && !busy && !noAdmin && void invite()}
          placeholder="GitHub username"
          aria-label="GitHub username to invite"
          disabled={!!noAdmin || busy}
          className="!py-1.5 text-sm"
        />
        <div className="w-36">
          <Select value={role} onChange={(v) => setRole(v as Role)} options={ROLES} disabled={!!noAdmin || busy} />
        </div>
        <Button size="sm" onClick={() => void invite()} disabled={!!noAdmin || busy || !username.trim()}>
          {busy ? <Loader2 size={14} className="animate-spin" /> : <UserPlus size={14} />}
          Invite
        </Button>
      </div>
      {noAdmin && <p className="text-xs text-amber-300/90">{noAdmin}</p>}
      {!noAdmin && !orgish && <p className="text-xs text-zinc-500">On a personal repository every collaborator can write; the role counts on organization repositories.</p>}

      {error && (
        <p role="alert" className="px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 text-red-400 text-xs whitespace-pre-line break-words">
          {error}
        </p>
      )}

      {people && (
        <ul className="space-y-1">
          {people.collaborators.map((c) => (
            <li key={c.login} data-collaborator={c.login} className="flex items-center gap-3 px-3 py-2 rounded-lg bg-zinc-800/40 border border-zinc-700/40 text-sm min-w-0">
              <span className="text-zinc-200 truncate flex-1 min-w-0">{c.login}</span>
              {c.is_owner && <Badge variant="success">owner</Badge>}
              <Badge>{c.role}</Badge>
              {!c.is_owner && (
                <Button size="sm" variant="ghost" onClick={() => setRemoving(c)} disabled={!!noAdmin || busy} aria-label={`Remove ${c.login}`} title={noAdmin ?? `Take ${c.login}'s access away`}>
                  <UserMinus size={14} />
                </Button>
              )}
            </li>
          ))}
          {people.collaborators.length === 0 && <li className="text-xs text-zinc-500">Not shared with anyone directly.</li>}
        </ul>
      )}

      {people?.invitations && people.invitations.length > 0 && (
        <div className="space-y-1">
          <h4 className="text-xs uppercase tracking-wider text-zinc-500">Waiting for them to accept</h4>
          <ul className="space-y-1">
            {people.invitations.map((inv) => (
              <li key={inv.id} data-invitation={inv.login} className="flex items-center gap-3 px-3 py-2 rounded-lg bg-zinc-800/30 border border-dashed border-zinc-700/60 text-sm min-w-0">
                <Mail size={14} className="text-zinc-500 shrink-0" />
                <span className="text-zinc-300 truncate flex-1 min-w-0">
                  {inv.login} <span className="text-xs text-zinc-500">invited {ago(inv.created_at)}{inv.inviter ? ` by ${inv.inviter}` : ""}</span>
                </span>
                <Badge>{inv.role}</Badge>
                <Button size="sm" variant="ghost" onClick={() => void cancel(inv)} disabled={!!noAdmin || busy} aria-label={`Cancel the invitation for ${inv.login}`}>
                  <X size={14} />
                </Button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {people?.note && <p className="text-xs text-zinc-500">{people.note}</p>}

      <Modal open={!!removing} onClose={() => !busy && setRemoving(null)} title={`Remove ${removing?.login ?? ""}?`}>
        {removing && (
          <div className="space-y-3 text-sm">
            <p className="text-zinc-300">
              {removing.login} loses access to {owner}/{name}. Their forks of a private repository are deleted by GitHub.
            </p>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setRemoving(null)} disabled={busy}>
                Cancel
              </Button>
              <Button variant="danger" onClick={() => void remove(removing)} disabled={busy}>
                {busy ? <Loader2 size={14} className="animate-spin" /> : <UserMinus size={14} />}
                Remove access
              </Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
