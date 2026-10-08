import { readPersisted, writePersisted } from "./persist";

/**
 * GitHub accounts signed in to with "Sign in with GitHub" (the device flow),
 * remembered here by their public facts — login, id, name, email, avatar —
 * so the profile form can fill an identity from them later, as it does from
 * a gh login. The sign-in's token is kept by the backend, in the system's
 * credential store (never here): with it the account lists its repositories
 * and registers keys, like a gh login.
 */
export type SignIn = {
  id: number;
  login: string;
  name: string | null;
  email: string | null;
  avatar_url: string;
  /** The account's primary verified address and every verified one, when the token could read them. */
  primary_email?: string | null;
  emails?: string[];
};

type Account = {
  id: number;
  login: string;
  name: string | null;
  email: string | null;
  primary_email?: string | null;
  emails?: string[];
  email_scope_missing?: boolean;
};

/**
 * The git identity an account stands for: its login, and its primary
 * verified address — else its public one, else the no-reply address GitHub
 * gives every account. The others are offered one click away.
 * `scopeMissing`: the token could not read the account's addresses, and
 * none is public, so only the no-reply one is known.
 */
export function identityOf(u: Account) {
  const noreply = `${u.id}+${u.login}@users.noreply.github.com`;
  const best = u.primary_email || u.email || noreply;
  return {
    login: u.login,
    email: best,
    names: Array.from(new Set([u.login, u.name].filter(Boolean) as string[])),
    emails: Array.from(new Set([u.primary_email, ...(u.emails ?? []), u.email, noreply].filter(Boolean) as string[])),
    scopeMissing: !!u.email_scope_missing && !u.primary_email && !u.email,
  };
}

const KEY = "github.signins";

function wellFormed(x: unknown): x is SignIn {
  const s = x as SignIn;
  return !!s && typeof s.id === "number" && typeof s.login === "string" && s.login.length > 0;
}

export function rememberedSignIns(): SignIn[] {
  const list = readPersisted<unknown>(KEY, []);
  return Array.isArray(list) ? list.filter(wellFormed) : [];
}

/** Newest first, one per login whatever its letter case. */
export function rememberSignIn(u: SignIn): void {
  const one: SignIn = {
    id: u.id,
    login: u.login,
    name: u.name ?? null,
    email: u.email ?? null,
    avatar_url: u.avatar_url ?? "",
    primary_email: u.primary_email ?? null,
    emails: u.emails ?? [],
  };
  const rest = rememberedSignIns().filter((s) => s.login.toLowerCase() !== u.login.toLowerCase());
  writePersisted(KEY, [one, ...rest]);
}

export function forgetSignIn(login: string): void {
  writePersisted(
    KEY,
    rememberedSignIns().filter((s) => s.login.toLowerCase() !== login.toLowerCase())
  );
}

/** The value a remembered sign-in has in an account picker, beside gh's plain logins. */
export const SIGNIN_PREFIX = "signin:";
export const isSignInValue = (v: string) => v.startsWith(SIGNIN_PREFIX);
export const loginOfValue = (v: string) => (isSignInValue(v) ? v.slice(SIGNIN_PREFIX.length) : v);
