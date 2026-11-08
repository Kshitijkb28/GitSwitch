/**
 * Path helpers that work for Windows (`C:\Users\me`) as well as Unix.
 * The backend stores whatever the OS gave it, so the UI must never assume "/".
 */

/** Forward slashes, no trailing separator — safe for comparisons. */
export function normPath(p: string): string {
  const slashed = p.replace(/\\/g, "/");
  const trimmed = slashed.replace(/\/+$/, "");
  return trimmed || slashed;
}

/** Last path component, whichever separator the platform used. */
export function baseName(p: string): string {
  const parts = normPath(p).split("/").filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

/** A repository path plus a submodule path inside it, or the repository alone —
 *  one spelling, so the job slot, drafts and the backend's status paths agree. */
export function canonicalJoin(path: string, sub: string): string {
  const base = normPath(path);
  const s = sub.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
  return s ? `${base}/${s}` : base;
}

/** Is `child` the same folder as `parent`, or inside it? */
export function isWithin(child: string, parent: string): boolean {
  const c = normPath(child);
  const p = normPath(parent);
  return c === p || c.startsWith(`${p}/`);
}
