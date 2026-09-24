// Repo states the Changes page has to handle, as the backend would report them.

export const REPOS = [
  { path: "/repos/gitswitch", name: "gitswitch", profile_id: "p1", profile_name: "Kirmada", profile_email: "me@personal.test" },
  { path: "/repos/argos", name: "argos", profile_id: "p2", profile_name: "Work", profile_email: "me@work.test" },
];

function entry(over = {}) {
  return {
    path: "src/app.ts",
    orig_path: null,
    staged: ".",
    unstaged: "M",
    kind: "tracked",
    conflict: null,
    rename_score: null,
    is_submodule: false,
    sub_commit_changed: false,
    sub_tracked_changes: false,
    sub_untracked: false,
    recorded_oid: null,
    staged_added: null,
    staged_removed: null,
    unstaged_added: 3,
    unstaged_removed: 1,
    is_binary: false,
    ...over,
  };
}

export const LOCK_CAVEATS = [
  "Turning this lock off needs an administrator: its rules live in files only an administrator can change, and GitSwitch will not remove them without the administrator prompt. Removing them by hand needs the same prompt.",
  "It stops `git push` — with or without `--no-verify` — from anything that uses this machine's system git in a normal environment: your terminal, editors, scripts and AI agents.",
  "It does not stop someone who deliberately works around it. Without any password, a user of this account can: set `GIT_CONFIG_NOSYSTEM=1` (or `GIT_CONFIG_SYSTEM=/dev/null`) so git ignores the system-wide rules; give the remote an explicit push URL; push to a spelling of the URL that no pinned prefix covers; supply their own `git-remote-gitswitch-push-blocked` program; use a different git; or move or copy the repository to another folder.",
  "This covers Apple's git (/usr/bin/git) through /etc/gitconfig, which survives Command Line Tools updates. A Homebrew git reads a different system file and is not covered.",
];

export function lock(over = {}) {
  return {
    supported: true,
    platform: "macos",
    locked: false,
    registry: "missing",
    system_include: "",
    stanza: "",
    system_rewrite_measured: false,
    helper: "missing",
    helper_installed: false,
    remote_helper: "missing",
    mirrors: "ok",
    drift: [],
    needs_elevation: false,
    caveats: [],
    locked_at: null,
    recent_events: [],
    system_gitconfig: "/etc/gitconfig",
    audit_log: "/etc/gitswitch/audit.log",
    bundled_helper_sha256: "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12",
    ...over,
  };
}

const LOCKED_REMOTE = {
  name: "origin",
  fetch_url: "git@github.com:me/gitswitch.git",
  push_url: "gitswitch-push-blocked://git@github.com:me/gitswitch.git",
  rewritten: true,
  has_explicit_pushurl: false,
};

/** A fully measured, healthy lock. */
export function lockedLock(over = {}) {
  return lock({
    locked: true,
    registry: "ok",
    system_include: "ok",
    stanza: "ok",
    system_rewrite_measured: true,
    helper: "ok",
    helper_installed: true,
    remote_helper: "ok",
    mirrors: "ok",
    caveats: LOCK_CAVEATS,
    locked_at: "2026-09-23T09:15:00Z",
    recent_events: [
      { ts: "2026-09-23T09:15:00Z", repo: "/repos/gitswitch", code: "locked", detail: "Locked 1 repository. Lock helper installed.", healed: false },
    ],
    ...over,
  });
}

function push(over = {}) {
  return {
    lock: lock(),
    profile_blocked: false,
    profile_name: "Kirmada",
    repo_blocked: false,
    blocked: false,
    reason: "Pushing is allowed from this folder.",
    hook: "none",
    hooks_path_overridden: null,
    config_block_present: false,
    remotes: [
      {
        name: "origin",
        fetch_url: "git@github.com:me/gitswitch.git",
        push_url: "git@github.com:me/gitswitch.git",
        rewritten: false,
        has_explicit_pushurl: false,
      },
    ],
    gaps: [],
    needs_repair: false,
    ...over,
  };
}

function identity(over = {}) {
  return {
    name: "Me",
    email: "me@personal.test",
    email_scope: "global",
    email_origin: "/home/me/.gitconfig",
    profile_id: "p1",
    profile_name: "Kirmada",
    profile_email: "me@personal.test",
    matches_profile: true,
    signing_on: false,
    signing_key: null,
    guard: "none",
    ...over,
  };
}

export function status(over = {}) {
  const base = {
    path: "/repos/gitswitch",
    name: "gitswitch",
    branch: "main",
    detached: false,
    unborn: false,
    head_oid: "aaaa1111",
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
    identity: identity(),
    push: push(),
    can_amend: true,
    head_subject: "the last commit",
    merge_message: null,
    has_submodules: false,
    uses_lfs: false,
    sync: null,
    conflict_source: null,
  };
  return { ...base, ...over };
}

// --- Sync: "the latest from upstream, your commits on top" ------------------
// Shaped like the sandbox fixture in scripts/verify/sync.sh: a superproject
// with submodules A (carrying my commit) and B (nobody's), an unmapped gitlink.
const SYNC_SUPER = {
  branch: "main",
  upstream: "origin/main",
  head: "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111",
  upstream_oid: "bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222",
  ahead: 2,
  behind: 3,
  own_commits: [
    { oid: "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", short: "1111111", subject: "local unrelated", touches: [] },
    { oid: "2222222aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", short: "2222222", subject: "seal work", touches: ["A"] },
  ],
  incoming: [
    { oid: "3333333aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", short: "3333333", subject: "super: bump B", touches: ["B"] },
    { oid: "4444444aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", short: "4444444", subject: "super: upstream work", touches: [] },
    { oid: "5555555aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", short: "5555555", subject: "super: bump A", touches: ["A"] },
  ],
  file_overlap: ["shared.txt"],
  dirty_count: 1,
  untracked: 1,
  will_stash: true,
  notes: [],
};
const SYNC_SUB_A = {
  path: "A", name: "A", action: "rebase",
  reason: "your 1 commit is rebased onto 7a9d38a when the superproject rebase stops on its pointer",
  head: "a535d05aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", recorded: "a535d05aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  target: "7a9d38aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", rebase_onto: "7a9d38aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  predicted_gitlink_conflict: true, branch: "main", detached: false, reattach_to: null,
  dirty_tracked: 1, untracked: 1, fetch_error: null,
  notes: ["its 1 uncommitted change is stashed and re-applied around the rebase"],
};
const SYNC_SUB_B = {
  path: "B", name: "B", action: "fast-forward", reason: "moves forward to 3c3c3c3 (no commits of yours)",
  head: "84d0861aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", recorded: "84d0861aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  target: "3c3c3c3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", rebase_onto: null,
  predicted_gitlink_conflict: false, branch: "main", detached: false, reattach_to: null,
  dirty_tracked: 0, untracked: 0, fetch_error: null, notes: [],
};
export const SYNC_PLAN = {
  can_run: true,
  nothing_to_do: false,
  refusal: null,
  blockers: [],
  superproject: SYNC_SUPER,
  submodules: [SYNC_SUB_A, SYNC_SUB_B],
  skipped: [{ path: "orphan", why: "unmapped" }],
  fingerprint: [["", SYNC_SUPER.upstream_oid], ["A", SYNC_SUB_A.target], ["B", SYNC_SUB_B.target]],
  summary: "Your 2 commits are replayed on top of 3 new commits from origin/main, rebasing the submodules that carry your work",
};
export const SYNC_PLAN_NOSTASH = {
  ...SYNC_PLAN,
  can_run: false,
  blockers: ["Turn on stashing, or commit or stash your 1 uncommitted change first"],
  superproject: { ...SYNC_SUPER, will_stash: false },
};
export const SYNC_PLAN_BLOCKED = {
  ...SYNC_PLAN,
  can_run: false,
  blockers: ["B: it is checked out at 2404a33 but this branch records 84d0861; commit that pointer (Record submodule pointers) or move it back before syncing"],
  submodules: [SYNC_SUB_A, { ...SYNC_SUB_B, action: "blocked", head: "2404a33aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", reason: "it is checked out at 2404a33 but this branch records 84d0861; commit that pointer (Record submodule pointers) or move it back before syncing" }],
};
export const SYNC_PLAN_NOTHING = {
  ...SYNC_PLAN,
  can_run: false,
  nothing_to_do: true,
  superproject: { ...SYNC_SUPER, ahead: 2, behind: 0, incoming: [], file_overlap: [] },
  submodules: [{ ...SYNC_SUB_A, action: "none", reason: "already at the commit upstream records", notes: [] }, { ...SYNC_SUB_B, action: "none", reason: "already at the commit upstream records" }],
  summary: "Nothing to sync: main already has everything from origin/main",
};
export const SYNC_OUTCOME = {
  phase: "done",
  needs_user: null,
  backups: [
    { repo: "/repos/gitswitch", branch: "gitswitch-before-sync-20260923120000", oid: SYNC_SUPER.head, recovery: "git -C /repos/gitswitch reset --hard gitswitch-before-sync-20260923120000" },
    { repo: "/repos/gitswitch/A", branch: "gitswitch-before-sync-20260923120000", oid: SYNC_SUB_A.head, recovery: "git -C /repos/gitswitch/A reset --hard gitswitch-before-sync-20260923120000" },
  ],
  bundles: [],
  head_before: SYNC_SUPER.head,
  head_after: "cccc3333cccc3333cccc3333cccc3333cccc3333",
  incoming: 3,
  own_kept: SYNC_SUPER.own_commits,
  own_dropped: [],
  stashed: true,
  stash_left: false,
  submodules: [
    { path: "A", action: "rebase", head_before: SYNC_SUB_A.head, head_after: "d1d1d1daaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", recorded_after: "d1d1d1daaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", ok: true, note: "rebased onto 7a9d38a; 1 commit of yours on top" },
    { path: "B", action: "fast-forward", head_before: SYNC_SUB_B.head, head_after: SYNC_SUB_B.target, recorded_after: SYNC_SUB_B.target, ok: true, note: "fast-forwarded to 3c3c3c3" },
  ],
  verified: { behind_zero: true, own_on_top: true, dirty_paths_unchanged: true, untracked_unchanged: true, submodules_aligned: true, no_conflicts_left: true },
  unrecorded_pointers: [],
};
export const SYNC_OUTCOME_UNRECORDED = {
  ...SYNC_OUTCOME,
  own_kept: [],
  incoming: 1,
  submodules: [{ path: "B", action: "ahead", head_before: "9b9b9b9aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", head_after: "9b9b9b9aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", recorded_after: SYNC_SUB_B.target, ok: true, note: "left alone; your commits sit on top of the recorded commit" }],
  unrecorded_pointers: ["B"],
};
export const SYNC_PAUSED = {
  phase: "rebasing",
  started_at: "2026-09-23T12:00:00Z",
  needs_user: { where_: "superproject", paths: ["shared.txt"], hint: "Resolve the conflicts listed on this page, stage them, then Continue sync." },
  root: "/repos/gitswitch",
  is_root: true,
};
export const SYNC_PAUSED_IN_SUB = {
  ...SYNC_PAUSED,
  needs_user: { where_: "submodule:A", paths: ["A.txt"], hint: "Rebasing A onto 7a9d38a stopped on a conflict inside it. Open it as its own repository, resolve and stage, then Continue sync." },
};


export const MERGE_OP = {
  kind: "merge",
  label: "Merge in progress",
  detail: "Finish it with one commit, or abort.",
  abort_command: "git merge --abort",
  continue_command: "git commit --no-edit",
  sides: { mine: "main", theirs: "the branch being merged in" },
};

export const REBASE_OP = {
  kind: "rebase",
  label: "Rebase in progress",
  detail: "step 2 of 3, main onto origin/main",
  abort_command: "git rebase --abort",
  continue_command: "git rebase --continue",
  sides: { mine: "your commit being replayed", theirs: "origin/main" },
};

export const SCENARIOS = {
  clean: status(),

  dirty: status({
    entries: [
      entry({ path: "src/staged.ts", staged: "M", unstaged: ".", staged_added: 10, staged_removed: 2 }),
      entry({ path: "src/both.ts", staged: "M", unstaged: "M" }),
      entry({ path: "src/mod.ts" }),
      entry({ path: "new file.txt", kind: "untracked", staged: ".", unstaged: "?", unstaged_added: null, unstaged_removed: null }),
      entry({ path: "logo.png", is_binary: true, unstaged_added: null, unstaged_removed: null }),
    ],
    staged_count: 2,
    unstaged_count: 3,
    untracked_count: 1,
  }),

  diverged: status({ ahead: 2, behind: 3 }),
  behindOnly: status({ behind: 4 }),

  blocked: status({
    push: push({
      repo_blocked: true,
      blocked: true,
      reason: "Blocked for this repository.",
      hook: "gitswitch",
      config_block_present: true,
      remotes: [
        {
          name: "origin",
          fetch_url: "git@github.com:me/gitswitch.git",
          push_url: "gitswitch-push-blocked://git@github.com:me/gitswitch.git",
          rewritten: true,
          has_explicit_pushurl: false,
        },
      ],
      gaps: [
        "`git push --no-verify` skips the hook; the URL rewrite still applies.",
        "This is a guard-rail, not a lock: anyone with a terminal can turn it off with one git command, and a more specific `pushInsteadOf` of their own — or `GIT_CONFIG_NOSYSTEM=1` — gets past it. For enforcement that needs the administrator password to remove, use the lock.",
      ],
    }),
  }),

  locked: status({
    push: push({
      repo_blocked: true,
      blocked: true,
      reason: "Locked for this repository. Unlocking needs the administrator password.",
      hook: "gitswitch",
      config_block_present: true,
      remotes: [LOCKED_REMOTE],
      gaps: ["`git push --no-verify` skips the hook; the URL rewrite still applies."],
      lock: lockedLock(),
    }),
  }),

  lockedHealed: status({
    push: push({
      repo_blocked: true,
      blocked: true,
      reason: "Locked for this repository. Unlocking needs the administrator password.",
      hook: "gitswitch",
      config_block_present: true,
      remotes: [LOCKED_REMOTE],
      lock: lockedLock({
        mirrors: "healed",
        recent_events: [
          { ts: "2026-09-23T10:02:11Z", repo: "/repos/gitswitch", code: "mirrors-drifted", detail: "mirror-flag, mirror-hook — re-applied", healed: true },
          { ts: "2026-09-23T09:15:00Z", repo: "/repos/gitswitch", code: "locked", detail: "Locked 1 repository.", healed: false },
        ],
      }),
    }),
  }),

  lockedNeedsRepair: status({
    push: push({
      repo_blocked: true,
      blocked: true,
      reason: "Locked for this repository. Unlocking needs the administrator password.",
      hook: "gitswitch",
      config_block_present: true,
      remotes: [LOCKED_REMOTE],
      lock: lockedLock({ helper: "missing", remote_helper: "missing", drift: ["helper-missing", "remote-helper-missing"], needs_elevation: true }),
    }),
  }),

  lockUnsupported: status({
    push: push({ lock: lock({ supported: false, platform: null }) }),
  }),

  profileBlocked: status({
    push: push({
      profile_blocked: true,
      blocked: true,
      reason: "Blocked by profile 'Work'. Allow pushes for that profile, or move this folder.",
      profile_name: "Work",
    }),
  }),

  needsRepair: status({
    push: push({
      repo_blocked: true,
      blocked: true,
      reason: "Blocked for this repository.",
      hook: "gitswitch",
      config_block_present: true,
      needs_repair: true,
      gaps: ["Pushing to 'upstream' isn't stopped by the URL rewrite. Only the pre-push hook stops it, and `git push --no-verify` skips hooks."],
    }),
  }),

  wrongIdentity: status({
    entries: [entry({ path: "src/a.ts", staged: "M", unstaged: "." })],
    staged_count: 1,
    identity: identity({
      email: "me@personal.test",
      profile_email: "me@work.test",
      matches_profile: false,
      email_scope: "local",
      email_origin: "/repos/argos/.git/config",
      profile_name: "Work",
    }),
  }),

  merging: status({ operation: MERGE_OP, merge_message: "Merge branch 'origin/main'", staged_count: 0, can_amend: false }),

  rebasing: status({ operation: REBASE_OP, staged_count: 1, can_amend: false }),

  rebasingConflicted: status({
    operation: REBASE_OP,
    entries: [entry({ path: "conflict.txt", kind: "conflicted", staged: "U", unstaged: "U", conflict: "both modified", stage_modes: ["100644", "100644", "100644"], stage_oids: ["a".repeat(40), "b".repeat(40), "c".repeat(40)] })],
    conflicted_count: 1,
    staged_count: 0,
    can_amend: false,
  }),

  conflicted: status({
    entries: [entry({ path: "conflict.txt", kind: "conflicted", staged: "U", unstaged: "U", conflict: "both modified" })],
    conflicted_count: 1,
    operation: MERGE_OP,
  }),

  unpublished: status({ upstream: null, ahead: 2, last_fetch_secs: null }),

  truncated: status({
    entries: [entry({ path: "junk.txt", kind: "untracked", staged: ".", unstaged: "?" })],
    untracked_count: 1,
    untracked_truncated: true,
  }),

  submodules: status({ has_submodules: true, submodule_dirty_count: 2 }),

  syncPaused: status({
    operation: REBASE_OP,
    sync: SYNC_PAUSED,
    entries: [entry({ path: "shared.txt", kind: "conflicted", staged: "U", unstaged: "U", conflict: "both modified" })],
    conflicted_count: 1,
    can_amend: false,
  }),
  syncPausedInSub: status({
    operation: REBASE_OP,
    sync: SYNC_PAUSED_IN_SUB,
    has_submodules: true,
    entries: [entry({ path: "A", kind: "conflicted", staged: "U", unstaged: "U", conflict: "both modified", is_submodule: true })],
    conflicted_count: 1,
    can_amend: false,
  }),
  // The submodule's own page while its superproject's sync is paused inside it.
  syncPausedChild: status({
    path: "/repos/gitswitch/A",
    name: "A",
    operation: REBASE_OP,
    sync: { ...SYNC_PAUSED_IN_SUB, is_root: false },
    entries: [entry({ path: "A.txt", kind: "conflicted", staged: "U", unstaged: "U", conflict: "both modified" })],
    conflicted_count: 1,
    can_amend: false,
  }),
  lfsRepo: status({ uses_lfs: true }),

  pushedAlready: status({
    entries: [entry({ path: "src/a.ts", staged: "M", unstaged: "." })],
    staged_count: 1,
    can_amend: false,
    head_subject: "already on the remote",
  }),

  // Checked out a commit, not a branch: nothing can be committed or synced here.
  detached: status({ detached: true, branch: null, head_oid: "1a2b3c4d5e6f7a8b9c0d1a2b3c4d5e6f7a8b9c0d", upstream: null }),

  withStashes: status({
    entries: [
      entry({ path: "src/staged.ts", staged: "M", unstaged: ".", staged_added: 10, staged_removed: 2 }),
      entry({ path: "src/both.ts", staged: "M", unstaged: "M" }),
      entry({ path: "src/mod.ts" }),
      entry({ path: "new file.txt", kind: "untracked", staged: ".", unstaged: "?", unstaged_added: null, unstaged_removed: null }),
    ],
    staged_count: 2,
    unstaged_count: 2,
    untracked_count: 1,
    stash_count: 2,
  }),

  // A superproject whose submodules A and B each have work of their own.
  subSections: status({
    has_submodules: true,
    submodule_dirty_count: 2,
    entries: [
      entry({ path: "A", is_submodule: true, sub_commit_changed: true, staged: ".", unstaged: "M", unstaged_added: null, unstaged_removed: null }),
      entry({ path: "B", is_submodule: true, sub_tracked_changes: true, staged: ".", unstaged: "M", unstaged_added: null, unstaged_removed: null }),
    ],
    unstaged_count: 2,
  }),

  // Conflicts left by `git stash apply`: no operation to continue or abort.
  stashConflict: status({
    entries: [entry({ path: "conflict.txt", kind: "conflicted", staged: "U", unstaged: "U", conflict: "both modified" })],
    conflicted_count: 1,
    operation: null,
    conflict_source: "stash",
    stash_count: 1,
  }),
};

export const DIFF_TEXT = {
  path: "src/mod.ts",
  staged: false,
  lines: [
    { kind: "meta", text: "diff --git a/src/mod.ts b/src/mod.ts" },
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

export const DIFF_BINARY = {
  path: "logo.png",
  staged: false,
  lines: [],
  is_binary: true,
  truncated: false,
  total_lines: 0,
  added: 0,
  removed: 0,
  empty_reason: "Binary file — no text diff to show.",
};

/** Submodules shaped like the real `argos`: 32 gitlinks, 5 of them mapped. */
function sub(over = {}) {
  return {
    path: "vendor/lib",
    name: "lib",
    url: "git@github.com:org/lib.git",
    configured_branch: "main",
    recorded: "dceaf0bd6574d6ea9a551a5374a0188d229b23e0",
    recorded_short: "dceaf0b",
    actual: "dceaf0bd6574d6ea9a551a5374a0188d229b23e0",
    actual_short: "dceaf0b",
    initialised: true,
    listed: true,
    ahead: 0,
    behind: 0,
    recorded_missing: false,
    moved_commits: [],
    more_moved: 0,
    own_branch: "main",
    own_upstream: "origin/main",
    own_ahead: 0,
    own_behind: 0,
    dirty_tracked: 0,
    dirty_untracked: 0,
    dirty_untracked_capped: false,
    state: "clean",
    summary: "up to date with what this repo records; on main",
    ...over,
  };
}

export const SUBMODULES = [
  sub({ path: "delivery", name: "delivery" }),
  sub({
    path: "trinity",
    name: "trinity",
    actual: "d9b93c59276bdd504f19838a15cf971aadc18912",
    actual_short: "d9b93c5",
    ahead: 10,
    own_behind: 11,
    moved_commits: [
      { short: "d9b93c5", subject: "Updated" },
      { short: "1a4425f", subject: "test(subject): seal a retained successor" },
    ],
    more_moved: 8,
    state: "moved",
    summary: "moved 10 commits ahead of what this repo records; on main, 11 behind origin/main",
  }),
  sub({
    path: "staging",
    name: "staging",
    dirty_untracked: 2000,
    dirty_untracked_capped: true,
    state: "dirty",
    summary: "2000+ untracked files inside; on main",
  }),
  ...Array.from({ length: 27 }, (_, i) =>
    sub({
      path: `forge-tooling/aggtest/pkg${i}`,
      name: null,
      url: null,
      configured_branch: null,
      initialised: false,
      listed: false,
      actual: null,
      actual_short: null,
      own_branch: null,
      own_upstream: null,
      state: "unmapped",
      summary: "not initialised, and it has no entry in .gitmodules, so git can't fetch it",
    })
  ),
];

/** The submodule's own status, for the expanded view. */
export const SUB_INNER = status({
  path: "/repos/gitswitch/staging",
  name: "staging",
  entries: [
    entry({ path: "inside/changed.ts", unstaged: "M" }),
    entry({ path: "inside/new.txt", kind: "untracked", staged: ".", unstaged: "?", unstaged_added: null, unstaged_removed: null }),
  ],
  unstaged_count: 1,
  untracked_count: 1,
});

/** LFS states, shaped like the real `argos`: 5 tracked, all still pointers. */
export const LFS_POINTERS = {
  installed: true,
  version: "git-lfs/3.8.0",
  uses_lfs: true,
  filters_configured: false,
  tracked: 5,
  pointers: 5,
  pointer_paths: [
    "touchstones/Kong__insomnia_dataset.jsonl",
    "touchstones/axios__axios_dataset.jsonl",
    "touchstones/expressjs__express_dataset.jsonl",
    "touchstones/iamkun__dayjs_dataset.jsonl",
    "touchstones/vuejs__core_dataset.jsonl",
  ],
  more_pointers: 0,
  summary: "5 of 5 LFS files are still a pointer stub — the real content hasn't been downloaded.",
};

export const LFS_PRESENT = {
  ...LFS_POINTERS,
  filters_configured: true,
  pointers: 0,
  pointer_paths: [],
  summary: "All 5 LFS files are present.",
};

export const LFS_NOT_INSTALLED = {
  ...LFS_POINTERS,
  installed: false,
  version: null,
  tracked: 0,
  pointers: 0,
  pointer_paths: [],
  summary: "This repository uses Git LFS, but git-lfs isn't installed on this Mac — large files will stay as pointer stubs until it is.",
};

// --- Branches, stashes, submodule sections, tree outcomes --------------------

function branch(over = {}) {
  return {
    name: "main",
    is_current: false,
    is_remote: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    tip: "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111",
    short_tip: "aaaa111",
    last_author: "Me",
    last_date: "2026-09-20T10:00:00Z",
    last_subject: "the last commit",
    ...over,
  };
}

/** Local branches as history_branches reports them (one remote-tracking ref mixed in). */
export const BRANCHES = [
  branch({ name: "main", is_current: true, upstream: "origin/main", ahead: 2, behind: 0 }),
  branch({ name: "feature/login", ahead: 3, tip: "abc1234abc1234abc1234abc1234abc1234abc12", short_tip: "abc1234", last_subject: "Add login form" }),
  branch({ name: "hotfix/typo", last_subject: "Fix a typo in the README" }),
  branch({ name: "old-experiment", last_date: "2025-11-02T09:00:00Z", last_subject: "Try the other parser" }),
  branch({ name: "origin/main", is_remote: true, tip: "bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222", short_tip: "bbbb222" }),
];

export const STASHES = [
  {
    index: 0,
    ref: "stash@{0}",
    oid: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    message: "fix header",
    branch: "main",
    date: "2026-09-23T12:00:00Z",
    tracked_files: 3,
    untracked_files: 1,
    is_autostash: false,
  },
  {
    index: 1,
    ref: "stash@{1}",
    oid: "cafebabecafebabecafebabecafebabecafebabe",
    message: "spike: a much longer stash message that goes on and on about what it was trying to do here",
    branch: "feature/login",
    date: "2026-09-21T08:30:00Z",
    tracked_files: 2,
    untracked_files: 0,
    is_autostash: false,
  },
];

export const STASH_DETAIL = {
  entry: STASHES[0],
  files: [
    { path: "src/header.ts", status: "M", added: 4, removed: 1 },
    { path: "src/nav.ts", status: "M", added: 2, removed: 2 },
    { path: "styles/header.css", status: "M", added: 9, removed: 0 },
    { path: "notes.txt", status: "untracked", added: null, removed: null },
  ],
};

/** A: on a branch, one commit ahead, two changed files. B: detached, mid-rebase, one conflict. */
export const SUB_STATUSES = [
  {
    path: "A",
    name: "A",
    listed: true,
    recorded: "a535d05aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    recorded_short: "a535d05",
    status: status({
      path: "/repos/gitswitch/A",
      name: "A",
      ahead: 1,
      entries: [
        entry({ path: "inside/a.ts", staged: "M", unstaged: ".", staged_added: 5, staged_removed: 1 }),
        entry({ path: "inside/very/long/path/that/keeps/going/and/going/to/a/file/with/a/long/name.ts" }),
      ],
      staged_count: 1,
      unstaged_count: 1,
    }),
  },
  {
    path: "B",
    name: "B",
    listed: true,
    recorded: "84d0861aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    recorded_short: "84d0861",
    status: status({
      path: "/repos/gitswitch/B",
      name: "B",
      branch: null,
      detached: true,
      upstream: null,
      operation: REBASE_OP,
      entries: [entry({ path: "inside/b.txt", kind: "conflicted", staged: "U", unstaged: "U", conflict: "both modified" })],
      conflicted_count: 1,
      can_amend: false,
    }),
  },
];

/** A submodule with nothing to do: summarised in one line, no section. */
export const SUB_QUIET = {
  path: "C",
  name: "C",
  listed: true,
  recorded: "0c0c0c0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  recorded_short: "0c0c0c0",
  status: status({ path: "/repos/gitswitch/C", name: "C" }),
};

/** The side card's row for A, so its "Show what changed" becomes a jump link. */
export const SUBMODULE_A = sub({
  path: "A",
  name: "A",
  recorded: "a535d05aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  recorded_short: "a535d05",
  actual: "9f1e2d3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  actual_short: "9f1e2d3",
  ahead: 1,
  moved_commits: [{ short: "9f1e2d3", subject: "work inside A" }],
  dirty_tracked: 2,
  state: "moved-and-dirty",
  summary: "moved 1 commit ahead of what this repo records; 2 changed files inside; on main",
});

export const TREE_RESET_OUTCOME = {
  op: "reset",
  head_before: "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111",
  head_after: "bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222",
  branch_before: "main",
  branch_after: "main",
  commit: null,
  parents: [],
  dropped: [
    { oid: "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", short: "1111111", subject: "local unrelated" },
    { oid: "2222222aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", short: "2222222", subject: "seal work" },
  ],
  dropped_total: 2,
  backup: {
    repo: "/repos/gitswitch",
    branch: "gitswitch-before-reset-20260923120000",
    oid: "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111",
    recovery: "git -C /repos/gitswitch reset --hard gitswitch-before-reset-20260923120000",
  },
  stash: { ref: "stash@{0}", oid: "feedfacefeedfacefeedfacefeedfacefeedface", message: "gitswitch: before reset to origin/main" },
  recovery: "git -C /repos/gitswitch reset --hard gitswitch-before-reset-20260923120000",
  submodule_mismatch: [],
  blocking_files: [],
  conflicts: [],
  mapping_note: null,
  deleted_untracked: 0,
  restored_tracked: 0,
};

export const STASH_DROP_OUTCOME = {
  action: "drop",
  entry: { ref: "stash@{0}", oid: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", message: "fix header" },
  stash_count: 1,
  conflicts: [],
  kept: false,
  recovery: "git -C /repos/gitswitch stash store -m 'fix header' deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
  not_stashed_submodules: [],
};

export const BRANCH_DELETE_OUTCOME = {
  ...TREE_RESET_OUTCOME,
  op: "delete-branch",
  head_after: TREE_RESET_OUTCOME.head_before,
  branch_before: "feature/login",
  branch_after: null,
  commit: { oid: "abc1234abc1234abc1234abc1234abc1234abc12", short: "abc1234", subject: "Add login form" },
  dropped: [],
  dropped_total: 3,
  backup: null,
  stash: null,
  recovery: "git -C /repos/gitswitch branch feature/login abc1234abc1234abc1234abc1234abc1234abc12",
};
