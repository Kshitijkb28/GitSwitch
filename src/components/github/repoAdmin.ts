// Words and rules shared by the visibility and delete dialogs, the Repository
// page's Settings section and the Repositories page's row menu, so each says
// the same thing the same way.
import { useEffect, useRef } from "react";

/** Escape calls `onEscape` while `active` (a dialog is open and nothing is running). */
export function useEscape(active: boolean, onEscape: () => void) {
  const latest = useRef(onEscape);
  latest.current = onEscape;
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") latest.current();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [active]);
}

/** The login in an account key ("gh:octo" or "signin:octo" → "octo"). */
export const loginOf = (account: string) => account.slice(account.indexOf(":") + 1);

/** "1 star", "3 stars". */
export const count = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

/**
 * A list as it is said: "main", "main and dev", "main, dev and fix", "main,
 * dev, fix and 2 more". In parts, so the names can be set apart from the words
 * between them: `{ name }` or `{ text }`.
 */
export function listParts(names: string[], max = 3): ({ name: string } | { text: string })[] {
  const shown = names.length > max ? names.slice(0, max) : names;
  const rest = names.length - shown.length;
  const out: ({ name: string } | { text: string })[] = [];
  shown.forEach((name, i) => {
    if (i > 0) out.push({ text: !rest && i === shown.length - 1 ? " and " : ", " });
    out.push({ name });
  });
  if (rest) out.push({ text: ` and ${rest} more` });
  return out;
}

/** What the person typed names owner/name, letter case and spaces around it aside — the backend's own rule. */
export const typedMatches = (typed: string, owner: string, name: string) => typed.trim().toLowerCase() === `${owner}/${name}`.toLowerCase();

/** "3 stars and 2 watchers", "3 stars", or null when it has neither. */
export function starsAndWatchers(f: { stars: number; watchers: number }): string | null {
  const parts = [f.stars > 0 ? count(f.stars, "star") : null, f.watchers > 0 ? count(f.watchers, "watcher") : null].filter(Boolean);
  return parts.length ? parts.join(" and ") : null;
}

export type Visibility = "public" | "private" | "internal";
export type Want = "public" | "private";

/** What the visibility dialog offers: the other one. An internal repository is offered private, the careful way. */
export const wantFor = (visibility: Visibility): Want => (visibility === "private" ? "public" : "private");

/**
 * Why what was picked ("Make public…", "Make private…") is not for this
 * repository as GitHub has it now, or null when it is. The row or card that
 * offered it was behind (changed on github.com since it was read), or the
 * repository is internal, which the listing shows as private.
 */
export function pickedBehind(f: { visibility: Visibility }, want: Want): string | null {
  // Said under the line naming it and who can see it now, so not repeated.
  if (f.visibility === want) return `It is already ${want} on GitHub, so there is nothing to change.`;
  if (f.visibility === "internal" && want === "public") return "GitSwitch can make an internal repository private, not public. To make it public, use its settings on GitHub.";
  return null;
}

/** Why who can see it can't be changed as this account, in either direction, or null when it can. */
export function visibilityBlock(r: { admin: boolean; archived: boolean; fork: boolean; parent?: string | null }, login: string): string | null {
  if (!r.admin) return `Only an admin can change who can see it, and @${login} isn't one.`;
  if (r.archived) return "It is archived on GitHub: unarchive it there first.";
  // GitHub: "For security reasons, you cannot change the visibility of a fork." Either way. Nor does
  // it follow its parent: a parent made private or public leaves its forks as they are, split off.
  if (r.fork) return r.parent ? `It is a fork of ${r.parent}, and GitHub doesn't let a fork's visibility change.` : "It is a fork, and GitHub doesn't let a fork's visibility change.";
  return null;
}

/** Why it can't be deleted as this account, or null when it can. */
export const deleteBlock = (admin: boolean, login: string): string | null => (admin ? null : `Only an admin can delete it, and @${login} isn't one.`);

type Restorable = { owner: string; owner_kind: "user" | "org"; fork: boolean; forks: number };

/**
 * Who can bring it back after a delete, said before it. GitHub keeps a
 * deleted repository for 90 days, unless it was part of a fork network. A
 * personal one is restored by its owner, from that account's settings; an
 * organization's only by an owner of the organization, from its settings.
 */
export function restoreNote(f: Restorable): string {
  if (f.fork || f.forks > 0) return "You won't be able to restore it yourself afterwards.";
  return f.owner_kind === "org"
    ? `GitSwitch can't undo this. GitHub keeps it for 90 days: an owner of ${f.owner} can restore it from ${f.owner}'s settings.`
    : `GitSwitch can't undo this. GitHub keeps it for 90 days: you can restore it from @${f.owner}'s settings on GitHub.`;
}

/**
 * What GitHub's answer to a delete says beyond "Deleted owner/name on
 * GitHub." — who can restore it, and from where — or the whole answer when
 * it is worded otherwise.
 */
export function restoreWords(message: string, fullName: string): string {
  const lead = `Deleted ${fullName} on GitHub. `;
  return message.toLowerCase().startsWith(lead.toLowerCase()) ? message.slice(lead.length) : message;
}

/** Its page on github.com. */
export const repoUrl = (owner: string, name: string) => `https://github.com/${owner}/${name}`;

/** Its settings page on github.com: GitHub shows it to admins only. */
export const settingsUrl = (owner: string, name: string) => `${repoUrl(owner, name)}/settings`;
