/**
 * Find repositories: which profile a clone found outside every profile folder
 * should belong to, and whether that is clear enough to add it unasked.
 *
 * Pure: the dialog hands in what the backend found, which GitHub account each
 * profile is, and what GitHub said each account may do — and gets back one
 * proposal per repository. Nothing here talks to the backend.
 */
import type { Profile } from "../types/profile";
import type { AccessCell, AccessMatrix, FoundRepo } from "./api";

/** Which GitHub account a profile is, as far as GitSwitch can tell. */
export interface ProfileLogin {
  /** Lowercased, to compare with GitHub's answers. */
  login: string | null;
  /** As shown. */
  display: string | null;
  /** "key": the account its SSH key signs in as · "saved": the one the profile names. */
  source: "key" | "saved" | null;
}

export interface Proposal {
  /** The profile proposed; "" for none. */
  profileId: string;
  /** The evidence points at exactly one profile: safe to add without asking. */
  clear: boolean;
  /** Why, in a few words. */
  reason: string;
  /** Only the proposed profile may be chosen: any other would not change whose email the commits carry. */
  only: boolean;
  /** Said when another profile is picked by hand. */
  warnOther?: string;
  /** About the repository, whatever is picked. */
  note?: string;
}

const repoKey = (owner: string, name: string) => `${owner}/${name}`.toLowerCase();

/** The repository pushes go to, when it is on GitHub. */
export function pushKey(r: FoundRepo): string {
  if (r.push_url) return r.push_owner && r.push_name ? repoKey(r.push_owner, r.push_name) : "";
  return r.owner && r.name ? repoKey(r.owner, r.name) : "";
}

/** Every "owner/name" GitHub must be asked about: each origin, and each push target. */
export function accessKeys(repos: FoundRepo[]): string[] {
  const out = new Set<string>();
  for (const r of repos) {
    if (r.gitdir_elsewhere || r.commits_as.via === "folder") continue;
    if (r.owner && r.name) out.add(repoKey(r.owner, r.name));
    const pk = pushKey(r);
    if (pk) out.add(pk);
  }
  return [...out];
}

/** The profiles' logins, lowercased for comparing. */
export function loginOf(logins: Record<string, ProfileLogin>, p: Profile): string | null {
  return logins[p.id]?.login ?? (p.github_login ? p.github_login.replace(/^@/, "").trim().toLowerCase() || null : null);
}

export function propose(
  repo: FoundRepo,
  profiles: Profile[],
  logins: Record<string, ProfileLogin>,
  matrix: AccessMatrix | null,
  /** owner (lowercase) -> the profile its other clones here use */
  ownerHints: Record<string, string> = {}
): Proposal {
  const byId = (id: string | null | undefined) => (id ? profiles.find((p) => p.id === id) ?? null : null);
  const nameOf = (id: string) => byId(id)?.name ?? "its profile";
  const shownLogin = (p: Profile) => logins[p.id]?.display ?? loginOf(logins, p) ?? p.name;
  const ca = repo.commits_as;
  const fallback = profiles.find((p) => p.is_default) ?? null;

  // 1. Whose email the commits carry is settled somewhere adding cannot change.
  if (repo.gitdir_elsewhere) {
    const p = byId(ca.profile_id) ?? fallback;
    return p
      ? { profileId: p.id, clear: true, only: true, reason: `a worktree: it commits as ${p.name}, like the checkout its git folder belongs to` }
      : { profileId: "", clear: false, only: false, reason: "a worktree: the checkout its git folder belongs to decides" };
  }
  if (ca.via === "folder" && byId(ca.profile_id)) {
    const id = ca.profile_id!;
    return { profileId: id, clear: true, only: true, reason: `inside ${nameOf(id)}'s folder, deeper than the page looks` };
  }
  const note = ca.via === "local" ? "it sets its own email in its .git/config, which wins over any profile — Doctor can remove that" : undefined;
  if (ca.via === "rule" && byId(ca.profile_id)) {
    const id = ca.profile_id!;
    return { profileId: id, clear: true, only: false, reason: `${nameOf(id)}'s remote rule`, warnOther: `overrides ${nameOf(id)}'s remote rule for this clone`, note };
  }

  // 2. Otherwise GitHub's word on each profile's account.
  const none = (reason: string, hint = true): Proposal => {
    const h = hint && repo.owner ? byId(ownerHints[repo.owner.toLowerCase()]) : null;
    return h
      ? { profileId: h.id, clear: false, only: false, reason: `${reason} · your other ${repo.owner} clones use ${h.name}`, note }
      : { profileId: "", clear: false, only: false, reason, note };
  };
  if (!repo.owner || !repo.name) return none("not on GitHub", false);
  if (!matrix) return none("not checked: no GitHub account to ask", false);
  const pk = pushKey(repo);
  if (repo.push_url && !pk) return none("pushes go to an address that is not on GitHub", false);
  const rk = repoKey(repo.owner, repo.name);
  const cells = (k: string) => matrix.access[k] ?? {};
  const cellOf = (k: string, p: Profile): AccessCell | undefined => {
    const l = loginOf(logins, p);
    return l ? cells(k)[l] : undefined;
  };
  // An answer that proves nothing (single sign-on, a rate limit…): what the
  // other answers point to is still shown, but never added unasked.
  const failed = Array.from(new Set([pk, rk]))
    .flatMap((k) => Object.entries(cells(k)))
    .filter(([, c]) => c.state === "unknown")
    .map(([login, c]) => `${login}${c.why ? `: ${c.why}` : ""}`);
  const failedNote = failed.length ? `check failed · ${Array.from(new Set(failed)).join("; ")}` : "";
  const withFailed = (r: string) => (failedNote ? `${r} · ${failedNote}` : r);

  const asked = new Set(matrix.accounts.map((a) => a.login.toLowerCase()));
  const owner = (repo.push_owner || repo.owner).toLowerCase();
  let pick: Profile | null = null;
  let reason = "";
  const pushers = profiles.filter((p) => p.allow_push && cellOf(pk, p)?.state === "visible" && cellOf(pk, p)!.push);
  if (pushers.length === 1) {
    pick = pushers[0];
    reason = `only ${shownLogin(pick)} can push to it`;
  } else if (pushers.length > 1) {
    const owners = pushers.filter((p) => loginOf(logins, p) === owner);
    if (owners.length === 1) {
      pick = owners[0];
      reason = `${shownLogin(pick)} owns it`;
    } else {
      const accounts = new Set(pushers.map((p) => loginOf(logins, p)));
      return none(withFailed(accounts.size === 1 ? `${pushers.length} profiles share the account that can push (${shownLogin(pushers[0])})` : "several profiles can push"));
    }
  } else {
    const readers = profiles.filter((p) => cellOf(rk, p)?.state === "visible");
    const isPrivate = Object.values(cells(rk)).some((c) => c.state === "visible" && c.private === true);
    if (readers.length === 0) return none(failedNote || (asked.size ? "no account can see it" : "no account was asked"));
    if (!isPrivate) return none(withFailed("anyone can read it, and no profile can push"));
    if (readers.length > 1) return none(withFailed("several profiles can read it, and none can push"));
    pick = readers[0];
    reason = `private: only ${shownLogin(pick)} can read it`;
  }
  if (failedNote) return { profileId: pick.id, clear: false, only: false, reason: withFailed(reason), note };
  // A profile GitHub could not be asked about may be the owner's own.
  const shadow = profiles.find((p) => p.id !== pick!.id && loginOf(logins, p) === owner && !asked.has(owner));
  if (shadow) return { profileId: pick.id, clear: false, only: false, reason: `${reason} · not checked: ${shadow.name} (${owner}) has no account GitSwitch can ask`, note };
  return { profileId: pick.id, clear: true, only: false, reason, note };
}
