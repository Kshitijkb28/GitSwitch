// Repo states the History page has to handle, as the backend would report them.
// Self-contained: the Changes fixtures in fixtures.mjs belong to another suite.

export const REPO = "/repos/gitswitch";

export const REPOS = [
  { path: REPO, name: "gitswitch", profile_id: "p1", profile_name: "Kirmada", profile_email: "me@personal.test" },
  { path: "/repos/argos", name: "argos", profile_id: "p2", profile_name: "Work", profile_email: "me@work.test" },
];

// Three commits: HEAD, the one the tests act on, and a merge below it.
export const C1 = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
export const C2 = "b2c3d4e5f60718293a4b5c6d7e8f9012345678a1";
export const C3 = "c3d4e5f60718293a4b5c6d7e8f9012345678a1b2"; // merge
export const P1 = "d4e5f60718293a4b5c6d7e8f9012345678a1b2c3";
export const P2 = "e5f60718293a4b5c6d7e8f9012345678a1b2c3d4";
const short = (h) => h.slice(0, 7);

function commit(hash, subject, over = {}) {
  return {
    hash,
    short: short(hash),
    parents: [],
    author_name: "Me",
    author_email: "me@personal.test",
    date: "2026-09-20T10:00:00+02:00",
    subject,
    refs: [],
    is_merge: false,
    lane: 0,
    parent_lanes: [0],
    active_lanes: [0],
    ...over,
  };
}

export const HISTORY_PAGE = {
  commits: [
    commit(C1, "Tidy the settings page", { parents: [C2], refs: ["HEAD -> main", "origin/main"] }),
    commit(C2, "Teach the clone page about LFS", { parents: [C3] }),
    commit(C3, "Merge branch 'feature/lfs'", { parents: [P1, P2], is_merge: true, parent_lanes: [0, 1] }),
  ],
  total: 3,
  offset: 0,
  limit: 50,
  has_more: false,
  max_lane: 0,
};

function branch(name, over = {}) {
  return {
    name,
    is_current: false,
    is_remote: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    tip: C1,
    short_tip: short(C1),
    last_author: "Me",
    last_date: "2026-09-20T10:00:00+02:00",
    last_subject: "Tidy the settings page",
    ...over,
  };
}

export const BRANCHES = [
  branch("main", { is_current: true, upstream: "origin/main" }),
  branch("feature/lfs", { tip: P2, short_tip: short(P2), last_subject: "Point at LFS" }),
  branch("origin/main", { is_remote: true }),
];

/** Nothing checked out as a branch — HEAD sits on a commit. */
export const BRANCHES_DETACHED = BRANCHES.map((b) => ({ ...b, is_current: false }));

export const SYNC_STATUS = {
  branch: "main",
  upstream: "origin/main",
  ahead: 0,
  behind: 0,
  last_fetch_secs: 120,
  incoming_rev: null,
  local_tip: { short: short(C1), subject: "Tidy the settings page", author: "Me", date: "2026-09-20T10:00:00" },
  remote_tip: { short: short(C1), subject: "Tidy the settings page", author: "Me", date: "2026-09-20T10:00:00" },
};

export const COMMIT_DETAIL = {
  hash: C2,
  short: short(C2),
  author_name: "Me",
  author_email: "me@personal.test",
  author_date: "2026-09-20T09:30:00+02:00",
  committer_name: "Me",
  committer_email: "me@personal.test",
  subject: "Teach the clone page about LFS",
  body: "Shows whether git-lfs is installed before the clone starts.",
  parents: [C3],
  refs: [],
  files: [
    { path: "src/app.ts", added: "3", removed: "1" },
    { path: "README.md", added: "10", removed: "0" },
    // numstat prints "-" for both columns of a binary file.
    { path: "logo.png", added: "-", removed: "-" },
  ],
  signature: "",
};

export const COMMIT_DETAIL_MERGE = {
  ...COMMIT_DETAIL,
  hash: C3,
  short: short(C3),
  subject: "Merge branch 'feature/lfs'",
  body: "",
  parents: [P1, P2],
  files: [{ path: "src/lfs.ts", added: "40", removed: "2" }],
};

export const PARENTS = [
  { oid: P1, short: short(P1), subject: "Fix the sidebar width" },
  { oid: P2, short: short(P2), subject: "Point at LFS" },
];

/** A TreeOutcome as the backend attaches it to every tree operation, refusals included. */
export function treeOutcome(over = {}) {
  return {
    op: "revert",
    head_before: C1,
    head_after: C1,
    branch_before: "main",
    branch_after: "main",
    commit: null,
    parents: [],
    dropped: [],
    dropped_total: 0,
    backup: null,
    stash: null,
    recovery: null,
    submodule_mismatch: [],
    blocking_files: [],
    conflicts: [],
    mapping_note: null,
    deleted_untracked: 0,
    restored_tracked: 0,
    ...over,
  };
}

/** C2 as `history_resolve` describes it: behind HEAD, four commits would leave main. */
export const RESOLVED = {
  input: C2,
  oid: C2,
  short: short(C2),
  subject: COMMIT_DETAIL.subject,
  author: "Me",
  date: "2026-09-20T09:30:00+02:00",
  is_merge: false,
  parents: [{ oid: C3, short: short(C3), subject: "Merge branch 'feature/lfs'" }],
  is_head: false,
  contained_in_head: true,
  dropped_if_reset: 4,
  would_drop_pushed: false,
  on_remote: true,
};

export const RESOLVED_PUSHED = { ...RESOLVED, would_drop_pushed: true };
export const RESOLVED_NOT_CONTAINED = { ...RESOLVED, contained_in_head: false, dropped_if_reset: 0 };

export const RESOLVED_MERGE = {
  ...RESOLVED,
  input: C3,
  oid: C3,
  short: short(C3),
  subject: COMMIT_DETAIL_MERGE.subject,
  is_merge: true,
  parents: PARENTS,
  dropped_if_reset: 5,
};

export const DIFF_TEXT = {
  path: "src/app.ts",
  staged: false,
  lines: [
    { kind: "meta", text: "diff --git a/src/app.ts b/src/app.ts" },
    { kind: "hunk", text: "@@ -1,3 +1,4 @@" },
    { kind: "context", text: " unchanged" },
    { kind: "del", text: "-removed line" },
    { kind: "add", text: "+added line" },
  ],
  is_binary: false,
  truncated: false,
  total_lines: 5,
  added: 1,
  removed: 1,
  empty_reason: null,
};

/** Only the parts of a RepoStatus the History page and its panel read. */
function repoStatus(over = {}) {
  return {
    path: REPO,
    name: "gitswitch",
    branch: "main",
    detached: false,
    unborn: false,
    head_oid: C1,
    upstream: "origin/main",
    ahead: 0,
    behind: 0,
    last_fetch_secs: 120,
    entries: [],
    staged_count: 0,
    unstaged_count: 0,
    untracked_count: 0,
    conflicted_count: 0,
    submodule_dirty_count: 0,
    untracked_truncated: false,
    stash_count: 0,
    operation: null,
    identity: {
      name: "Me", email: "me@personal.test", email_scope: "global", email_origin: "/home/me/.gitconfig",
      profile_id: "p1", profile_name: "Kirmada", profile_email: "me@personal.test", matches_profile: true,
      signing_on: false, signing_key: null, guard: "none",
    },
    push: { blocked: false, reason: "Pushing is allowed from this folder.", remotes: [], gaps: [], needs_repair: false },
    can_amend: true,
    head_subject: "Tidy the settings page",
    merge_message: null,
    has_submodules: false,
    uses_lfs: false,
    sync: null,
    ...over,
  };
}

export const STATUS_CLEAN = repoStatus();
export const STATUS_DIRTY = repoStatus({ unstaged_count: 2, untracked_count: 1 });
export const STATUS_DETACHED = repoStatus({ branch: null, detached: true, head_oid: C2 });
