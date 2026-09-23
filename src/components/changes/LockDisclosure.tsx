import { ShieldCheck, ShieldAlert, ShieldX, History } from "lucide-react";
import type { LockState } from "../../lib/api";

/** One admin-owned layer of the lock, as measured — never assumed. */
type Layer = { label: string; state: "ok" | "warn" | "bad"; note: string };

export function layers(lock: LockState): Layer[] {
  const out: Layer[] = [];
  out.push(
    lock.system_include === "ok" && lock.stanza === "ok" && lock.system_rewrite_measured
      ? { label: "System-wide rule", state: "ok", note: "git itself reports the rewrite at system scope, from the administrator-owned file." }
      : lock.stanza === "stale"
        ? { label: "System-wide rule", state: "warn", note: "In place, but written for a different set of remotes. Repair rewrites it for the remotes this repository has now." }
        : { label: "System-wide rule", state: "bad", note: `Not what it should be (${[lock.system_include !== "ok" ? `include ${lock.system_include}` : null, lock.stanza !== "ok" ? `rule ${lock.stanza}` : null, !lock.system_rewrite_measured ? "git does not see it" : null].filter(Boolean).join(", ")}).` }
  );
  out.push(
    lock.helper === "ok"
      ? { label: "Lock helper", state: "ok", note: "Installed in an administrator-only folder; its checksum matches the registry." }
      : lock.helper === "outdated"
        ? { label: "Lock helper", state: "warn", note: "Works, but this GitSwitch ships a newer one. Upgrading needs the administrator prompt once." }
        : lock.helper === "missing"
          ? { label: "Lock helper", state: "bad", note: "Gone. The lock still holds, but it cannot be changed until the helper is reinstalled." }
          : { label: "Lock helper", state: "bad", note: "Present, but not the one the registry vouches for." }
  );
  out.push(
    lock.remote_helper === "ok"
      ? { label: "Refusal message", state: "ok", note: "A refused push prints GitSwitch's explanation, addressed to people and agents." }
      : lock.remote_helper === "unprotected-dir"
        ? { label: "Refusal message", state: "warn", note: "Installed in a folder this account can write to (a Homebrew-owned /usr/local/bin). Enforcement does not depend on it." }
        : { label: "Refusal message", state: "warn", note: "Not installed, so a refused push shows git's generic error instead. Enforcement does not depend on it." }
  );
  out.push(
    lock.mirrors === "ok"
      ? { label: "Repository mirrors", state: "ok", note: "The repository's own flag, rewrite and pre-push hook are in place." }
      : lock.mirrors === "healed"
        ? { label: "Repository mirrors", state: "warn", note: "Something removed them since the last look; GitSwitch put them back just now." }
        : lock.mirrors === "unfixable"
          ? { label: "Repository mirrors", state: "warn", note: "core.hooksPath sends hooks elsewhere, so GitSwitch will not write its hook here. A remote with an explicit push URL is then stopped by nothing." }
          : { label: "Repository mirrors", state: "bad", note: "Missing and could not be re-applied." }
  );
  return out;
}

export function LockDisclosure({ lock }: { lock: LockState }) {
  return (
    <div className="space-y-2">
      <ul className="space-y-1">
        {layers(lock).map((l) => {
          const Icon = l.state === "ok" ? ShieldCheck : l.state === "warn" ? ShieldAlert : ShieldX;
          const color = l.state === "ok" ? "text-emerald-400" : l.state === "warn" ? "text-amber-400" : "text-red-400";
          return (
            <li key={l.label} className="flex items-start gap-2 text-xs min-w-0">
              <Icon size={13} className={`shrink-0 mt-0.5 ${color}`} />
              <span className="min-w-0">
                <span className="text-zinc-300">{l.label}</span>
                <span className="text-zinc-500"> — {l.note}</span>
              </span>
            </li>
          );
        })}
      </ul>

      {lock.recent_events.length > 0 && (
        <details className="group">
          <summary className="text-xs text-zinc-500 cursor-pointer hover:text-zinc-400 select-none inline-flex items-center gap-1.5">
            <History size={12} /> Recent events
          </summary>
          <ul className="mt-1.5 space-y-1 pl-3">
            {lock.recent_events.map((e, i) => (
              <li key={i} className="text-xs text-zinc-500 leading-relaxed break-words">
                <span className="font-mono text-zinc-600">{e.ts.replace("T", " ").replace("Z", "")}</span>{" "}
                <span className={e.healed ? "text-amber-300/90" : "text-zinc-400"}>{e.code}</span>{" "}
                {e.detail}
              </li>
            ))}
          </ul>
          <p className="mt-1.5 text-xs text-zinc-600 leading-relaxed pl-3">
            This log is in your own account's files and can be edited without a password; the administrator-only record is{" "}
            <span className="font-mono">{lock.audit_log ?? "the helper's audit log"}</span>.
          </p>
        </details>
      )}

      {lock.caveats.length > 0 && (
        <details className="group">
          <summary className="text-xs text-zinc-500 cursor-pointer hover:text-zinc-400 select-none">
            What this doesn't stop
          </summary>
          <ul className="mt-1.5 space-y-1.5 pl-3">
            {lock.caveats.map((c, i) => (
              <li key={i} className="text-xs text-zinc-500 leading-relaxed list-disc break-words">
                {c}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
