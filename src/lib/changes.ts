// What a save changed, in a few lines for the confirmation. Compared on what
// the backend stored (trimmed, `~` expanded, folders spelled as the disk
// spells them), never on what was typed, so a reformatted value is not a change.

import type { Profile } from "../types/profile";
import * as api from "./api";
import { baseName } from "./paths";

/** Windows and macOS don't tell folder names apart by letter case. */
const caseBlind = typeof navigator === "undefined" || !/linux/i.test(navigator.userAgent);

/** The same folder, however it is spelled: slashes, a trailing one, letter case where the system ignores it. */
export function sameFolder(a: string, b: string): boolean {
  const norm = (p: string) => {
    const s = p.replace(/\\/g, "/").replace(/\/+$/, "");
    return caseBlind ? s.toLowerCase() : s;
  };
  return norm(a) === norm(b);
}

/** Signing takes effect only with a key. */
const signs = (p: Profile) => p.signing_enabled && !!p.ssh_key_path;
/** Pushing HTTPS clones over SSH takes effect only when pushes are allowed and there is a key. */
const overSsh = (p: Profile) => !!p.push_over_ssh && p.allow_push && !!p.ssh_key_path;

/**
 * The lines a profile's save confirmation lists. `before` is the profile as
 * stored before the save (none: it was just created); `others` every other
 * profile as stored before it, to say where a folder was taken from (a folder
 * belongs to one profile, so saving moves it).
 */
export function profileChanges(before: Profile | null, after: Profile, others: Profile[] = []): string[] {
  const movedFrom = (dir: string) => {
    const from = others.find((o) => o.id !== after.id && o.directories.some((d) => sameFolder(d, dir)));
    return from ? ` (moved here from “${from.name}”)` : "";
  };
  const out: string[] = [];

  if (!before) {
    out.push(`Commits as ${after.git_name} <${after.git_email}>`);
    if (after.ssh_key_path) out.push(`SSH key: ${after.ssh_key_path}`);
    for (const d of after.directories) out.push(`Folder: ${d}${movedFrom(d)}`);
    for (const r of after.remote_patterns ?? []) out.push(`Remote rule: ${r}`);
    if (after.github_login) out.push(`GitHub account: @${after.github_login}`);
    if (!after.allow_push) out.push("Pushes are blocked");
    if (overSsh(after)) out.push("HTTPS clones push over SSH with this key");
    if (signs(after)) out.push("Commits are signed");
    if (after.is_default) out.push("The default profile");
    return out;
  }

  if (before.name !== after.name) out.push(`Name: ${before.name} → ${after.name}`);
  if (before.git_name !== after.git_name) out.push(`Commit name: ${before.git_name} → ${after.git_name}`);
  if (before.git_email !== after.git_email) out.push(`Commit email: ${before.git_email} → ${after.git_email}`);

  const keyBefore = before.ssh_key_path || null;
  const keyAfter = after.ssh_key_path || null;
  if (keyBefore !== keyAfter) {
    out.push(keyAfter ? (keyBefore ? `SSH key: ${keyBefore} → ${keyAfter}` : `SSH key set: ${keyAfter}`) : `SSH key removed (was ${keyBefore})`);
  }

  for (const d of after.directories) if (!before.directories.some((b) => sameFolder(b, d))) out.push(`Folder added: ${d}${movedFrom(d)}`);
  for (const d of before.directories) if (!after.directories.some((a) => sameFolder(a, d))) out.push(`Folder removed: ${d}`);

  const rulesBefore = before.remote_patterns ?? [];
  const rulesAfter = after.remote_patterns ?? [];
  for (const r of rulesAfter) if (!rulesBefore.includes(r)) out.push(`Remote rule added: ${r}`);
  for (const r of rulesBefore) if (!rulesAfter.includes(r)) out.push(`Remote rule removed: ${r}`);

  const loginBefore = before.github_login || null;
  const loginAfter = after.github_login || null;
  if (loginBefore !== loginAfter) {
    out.push(loginAfter ? (loginBefore ? `GitHub account: @${loginBefore} → @${loginAfter}` : `GitHub account: @${loginAfter}`) : `GitHub account removed (was @${loginBefore})`);
  }

  if (before.allow_push !== after.allow_push) out.push(after.allow_push ? "Pushes allowed again" : "Pushes blocked");
  if (overSsh(before) !== overSsh(after)) out.push(overSsh(after) ? "HTTPS clones now push over SSH with this key" : "HTTPS clones no longer push over SSH");
  if (signs(before) !== signs(after)) out.push(signs(after) ? "Commit signing turned on" : "Commit signing turned off");
  if (!before.is_default && after.is_default) out.push("Now the default profile");
  return out;
}

/**
 * For each folder a save added that git doesn't know as a repository, a line
 * saying so, so that where it shows on the Repository page is no surprise.
 * A folder that can't be asked about adds nothing.
 */
export async function folderNotes(before: Profile | null, after: Profile): Promise<string[]> {
  const added = after.directories.filter((d) => !before?.directories.some((b) => sameFolder(b, d)));
  const notes = await Promise.all(
    added.map(async (d) => {
      try {
        const s = await api.folderState(d);
        if (s.kind === "no-repo") return `${baseName(d)} holds no git repository yet. It's on the Repository page, where you can make it one.`;
        if (s.kind === "inside-repo") return `${baseName(d)} is inside the repository ${s.top}, so this profile's identity doesn't apply there.`;
      } catch {
        // Not answerable now: nothing to add.
      }
      return null;
    })
  );
  return notes.filter((n): n is string => !!n);
}
