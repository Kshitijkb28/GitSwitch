import * as api from "../../lib/api";

/**
 * What the Publish dialog shows first, kept between openings so the form is
 * drawn with its accounts already in place: a folder's accounts (read from
 * local files, kept two minutes and refreshed behind each opening) and an
 * account's owners (asked of GitHub, kept ten minutes). Only logins and
 * organization names are kept, never a token.
 */
const ACCOUNTS_TTL = 2 * 60_000;
const OWNERS_TTL = 10 * 60_000;

type Entry<T> = { at: number; value: T };
const accounts = new Map<string, Entry<api.PublishAccounts>>();
const owners = new Map<string, Entry<api.RepoOwners>>();
const asking = new Map<string, Promise<unknown>>();

function fresh<T>(e: Entry<T> | undefined, ttl: number): T | null {
  return e && Date.now() - e.at < ttl ? e.value : null;
}

/** One request per key at a time: a second asker gets the first one's answer. */
function once<T>(key: string, run: () => Promise<T>): Promise<T> {
  const cur = asking.get(key) as Promise<T> | undefined;
  if (cur) return cur;
  const p = run().finally(() => asking.delete(key));
  asking.set(key, p);
  return p;
}

export function cachedAccounts(repoPath: string): api.PublishAccounts | null {
  return fresh(accounts.get(repoPath), ACCOUNTS_TTL);
}

/** Read again (cheap: local files only) and kept. */
export function loadAccounts(repoPath: string): Promise<api.PublishAccounts> {
  return once(`accounts:${repoPath}`, () =>
    api.githubPublishAccounts(repoPath).then((v) => {
      accounts.set(repoPath, { at: Date.now(), value: v });
      return v;
    })
  );
}

export function cachedOwners(account: string): api.RepoOwners | null {
  return fresh(owners.get(account), OWNERS_TTL);
}

/** The kept answer while it is fresh, else GitHub's. */
export function loadOwners(account: string): Promise<api.RepoOwners> {
  const hit = cachedOwners(account);
  if (hit) return Promise.resolve(hit);
  return once(`owners:${account}`, () =>
    api.githubOwners(account).then((v) => {
      owners.set(account, { at: Date.now(), value: v });
      return v;
    })
  );
}

/** Before the click, for a folder with nothing to publish to yet: its accounts (local files only). */
export function prefetchPublish(repoPath: string): void {
  if (!cachedAccounts(repoPath)) loadAccounts(repoPath).catch(() => undefined);
}
