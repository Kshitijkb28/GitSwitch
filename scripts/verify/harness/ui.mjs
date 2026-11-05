// Headless checks for the Changes page against a mocked backend.
// Serves the built app and replaces Tauri's invoke bridge before any app code
// runs, so every repo state can be shown without touching a real repository.
import fs from "node:fs";
import { launch, serve, tally, DIST_OK } from "./browser.mjs";
import {
  REPOS,
  SCENARIOS,
  DIFF_TEXT,
  DIFF_BINARY,
  SUBMODULES,
  SUB_INNER,
  LFS_POINTERS,
  LFS_LISTING,
  LFS_PRESENT,
  LFS_NOT_INSTALLED,
  SYNC_PLAN,
  SYNC_PLAN_NOSTASH,
  SYNC_PLAN_DROPPED,
  SYNC_PLAN_LEVEL,
  SYNC_PLAN_MANY,
  MERGE_OP,
  SYNC_PLAN_BLOCKED,
  SYNC_PLAN_NOTHING,
  SYNC_OUTCOME,
  SYNC_OUTCOME_UNRECORDED,
  BRANCHES,
  STASHES,
  STASH_DETAIL,
  SUB_STATUSES,
  SUB_QUIET,
  SUB_SYNC_PAUSED,
  STASHES_CAPPED,
  SUBMODULE_A,
  TREE_RESET_OUTCOME,
  STASH_DROP_OUTCOME,
  BRANCH_DELETE_OUTCOME, sub, PULL_MOVED_OUTCOME, CONFLICT_VIEW, WORKSPACE_ONE, WORKSPACE_TWO, WORKSPACE_TWO_DIRTY, WORKSPACE_TWO_OFFSET } from "./fixtures.mjs";

if (!DIST_OK) {
  console.error("dist/ is missing — run `npm run build` first");
  process.exit(1);
}

const t = tally();
const { ok, section } = t;

/** Install the mock bridge and open the Changes page in a given repo state. */
async function openPage(browser, server, scenario, opts = {}) {
  const cfg = {
    width: opts.width ?? 1280,
    pullMode: opts.pullMode ?? null,
    hang: opts.hang ?? false,
    submodules: opts.submodules ?? [],
    subInner: opts.subInner ?? null,
    lfs: opts.lfs ?? null,
    repos: opts.repos ?? null,
    pushState: opts.pushState ?? null,
    /** How far the block reaches into the submodules; null → none to speak of. */
    pushCoverage: opts.pushCoverage ?? null,
    /** Folders deleted since they were listed or remembered: path_exists says no, and a shell cannot start there. */
    gonePaths: opts.gonePaths ?? [],
    /** Folders that are there, but where a shell fails to start. */
    terminalFails: opts.terminalFails ?? [],
    /** What Repair (locked) answers; null → "Everything is in place." */
    repairLockResult: opts.repairLockResult ?? null,
    pushResult: opts.pushResult ?? null,
    subPushResult: opts.subPushResult ?? null,
    consoleEntries: opts.consoleEntries ?? [],
    lfsListing: opts.lfsListing ?? LFS_LISTING,
    lfsProgress: opts.lfsProgress ?? null,
    lockOutcome: opts.lockOutcome ?? "applied",
    syncPlan: opts.syncPlan ?? SYNC_PLAN,
    syncPlanNoStash: opts.syncPlanNoStash ?? SYNC_PLAN_NOSTASH,
    // The plan after a remedy ran (pointers recorded): the real backend plans
    // on the new HEAD and the blocker is gone.
    syncPlanAfterRemedy: opts.syncPlanAfterRemedy ?? null,
    /** The plan from the second assessment on (upstream moved in between, say). */
    syncPlanSecond: opts.syncPlanSecond ?? null,
    syncOutcome: opts.syncOutcome ?? SYNC_OUTCOME,
    /** A pull's full result (with its `pull` outcome), when the test needs one. */
    pullResult: opts.pullResult ?? null,
    /** What Update now returns instead of the outcome (a refusal, say). */
    syncRunResult: opts.syncRunResult ?? null,
    conflictView: opts.conflictView ?? CONFLICT_VIEW,
    /** What changes_stage answers unless markers are allowed. */
    stageRefusal: opts.stageRefusal ?? null,
    /** The same for changes_continue and changes_sync_continue. */
    continueRefusal: opts.continueRefusal ?? null,
    syncContinueRefusal: opts.syncContinueRefusal ?? null,
    /** Update now does not answer until the test calls window.__RELEASE_RUN__(result). */
    syncRunHold: opts.syncRunHold ?? false,
    submoduleStatuses: opts.submoduleStatuses ?? [],
    branches: opts.branches ?? BRANCHES,
    stashes: opts.stashes ?? [],
    stashDetail: opts.stashDetail ?? STASH_DETAIL,
    /** {[cmd]: {code, message, guidance}} — that command answers with a refusal. */
    refuse: opts.refuse ?? {},
    autostash: opts.autostash ?? false,
    treeReset: opts.treeReset ?? TREE_RESET_OUTCOME,
    /** The tree outcome changes_create_branch answers with (null: none). */
    createBranchTree: opts.createBranchTree ?? null,
    stashDrop: STASH_DROP_OUTCOME,
    branchDelete: BRANCH_DELETE_OUTCOME,
    latency: Number(opts.latency ?? process.env.UI_LATENCY ?? 0) || 0,
    /** Who a push would authenticate as; null → the default fixture. */
    pushIdentity: opts.pushIdentity ?? null,
    /** The clone page: what git is doing, and how big GitHub says the repo is. */
    cloneProgress: opts.cloneProgress ?? null,
    cloneSize: opts.cloneSize ?? null,
    /** How long the shell takes to start: the pty answers after this many ms. */
    terminalOpenDelayMs: opts.terminalOpenDelayMs ?? 0,
    /** More remembered settings (gitswitch:* keys, without the prefix). */
    storage: opts.storage ?? null,
    /** What workspace_info answers: by default a repository cloned once. */
    workspace: opts.workspace ?? WORKSPACE_ONE,
    /** What ports_list answers on the Repository page; null → the listing is unread. */
    ports: opts.ports ?? null,
    /** workspace_set_offset refuses with this message (a clash with another copy). */
    offsetRefusal: opts.offsetRefusal ?? null,
    /**
     * Find repositories: {result, profiles, keyLogins, matrix, hints, pick, hold}.
     * `hold`: find_unlisted_repos answers only when the test calls window.__RELEASE_FIND__().
     */
    find: opts.find ?? null,
    /**
     * GitHub (the Repository page's GitHub tab and the Pull requests page):
     * {context, accounts, pulls, search, detail, files, commits, people, reviewRefuse, mergeRefuse}.
     * Merging, inviting and removing change it, as GitHub would.
     */
    gh: opts.gh ?? null,
    /** What changes_file_diff answers for a text file; null → DIFF_TEXT. */
    diffText: opts.diffText ?? null,
  };
  // Named, so another engine can be handed the very same bridge (webkit.sh).
  const bridge = (
    (repos, status, diffText, diffBinary, cfg) => {
      window.__CALLS__ = [];
      window.__SCENARIO__ = status;
      localStorage.clear();
      localStorage.setItem("gitswitch:changes.repo", JSON.stringify("/repos/gitswitch"));
      for (const [k, v] of Object.entries(cfg.storage ?? {})) localStorage.setItem(`gitswitch:${k}`, JSON.stringify(v));
      if (cfg.pullMode) {
        localStorage.setItem("gitswitch:changes.pullMode", JSON.stringify(cfg.pullMode));
      }
      if (cfg.autostash) {
        localStorage.setItem("gitswitch:changes.pullAutostash", JSON.stringify(true));
      }
      // An operation inside a submodule answers with that submodule's status.
      const statusFor = (p) =>
        (p && p !== "/repos/gitswitch" && (cfg.submoduleStatuses.find((s) => s.status.path === p) || {}).status) ||
        window.__SCENARIO__;
      const done = (headline, p) =>
        Promise.resolve({ ok: true, headline, detail: "", status: statusFor(p) });
      window.__TAURI_INTERNALS__ = {
        // A Channel (the terminal's output stream) is built on this. Without
        // it, constructing one throws and the drawer never starts a shell.
        transformCallback: (callback) => {
          const id = (window.__NEXT_CB__ = (window.__NEXT_CB__ || 0) + 1);
          (window.__CHANNELS__ = window.__CHANNELS__ || {})[id] = callback;
          return id;
        },
        // UI_LATENCY=<ms> delays every answer: a mock that answers in the same
        // tick hides exactly the races a real backend produces (a result for a
        // repository the page has since left, a request cancelled by the state
        // it set). The default is instant, as before.
        invoke: (cmd, args) => {
          const answer = window.__TAURI_INTERNALS__.answer(cmd, args);
          if (!cfg.latency) return answer;
          return new Promise((res, rej) => setTimeout(() => Promise.resolve(answer).then(res, rej), cfg.latency));
        },
        answer: (cmd, args) => {
          window.__CALLS__.push({ cmd, args });
          // Forcing a delete is the answer to its own refusal, so it goes through.
          const refusal = cfg.refuse[cmd];
          if (refusal && !(cmd === "changes_delete_branch" && args && args.force)) {
            return Promise.resolve({
              ok: false,
              headline: refusal.message,
              detail: "",
              refusal: { code: refusal.code, message: refusal.message },
              advice: { headline: refusal.message, guidance: refusal.guidance ?? "", action: null, git_said: "" },
              status: statusFor(args && args.repoPath),
            });
          }
          switch (cmd) {
            case "history_list_repos":
              // A test clones one "behind the app's back" by setting window.__REPOS__.
              return Promise.resolve(window.__REPOS__ ?? cfg.repos ?? repos);
            case "changes_repo_status": {
              const sub = args.repoPath && cfg.submoduleStatuses.find((s) => s.status.path === args.repoPath);
              if (sub) return Promise.resolve(sub.status);
              // A submodule path asks for that submodule's own status.
              return Promise.resolve(
                args.repoPath && args.repoPath.includes("/staging") && cfg.subInner
                  ? cfg.subInner
                  : window.__SCENARIO__
              );
            }
            case "changes_submodule_statuses":
              return Promise.resolve(cfg.submoduleStatuses);
            case "changes_push_identity":
              return Promise.resolve(cfg.pushIdentity ?? { remote: "origin", url: "git@github.com:octo-work/gitswitch.git", via: "ssh-key", login: "octo-work", key_path: "/Users/me/.ssh/id_work", host: "github.com", account: null, expected_login: null, detail: "github.com accepted the key id_work." });
            case "history_authors":
              return Promise.resolve([{ name: "Me", email: "me@example.test", commits: 2 }]);
            case "clone_progress":
              // A test ends a download part-way by setting window.__NO_PROGRESS__.
              return Promise.resolve(window.__NO_PROGRESS__ ? null : cfg.cloneProgress ?? null);
            case "clone_size_estimate":
              return Promise.resolve(cfg.cloneSize ?? { repo: "owner/repo", kb: 1258291 });
            case "history_head":
              return Promise.resolve({ branch: window.__SCENARIO__.branch, detached: window.__SCENARIO__.detached, unborn: window.__SCENARIO__.unborn });
            case "history_branches":
              return Promise.resolve(cfg.branches);
            case "changes_stash_list":
              return Promise.resolve(cfg.stashes);
            case "changes_stash_show":
              return Promise.resolve(cfg.stashDetail);
            case "changes_reset":
              return done("Reset main to origin/main.", args.repoPath).then((r) => ({ ...r, tree: cfg.treeReset }));
            case "changes_create_branch":
              return done("create_branch ok", args.repoPath).then((r) => (cfg.createBranchTree ? { ...r, tree: cfg.createBranchTree } : r));
            case "changes_stash_drop":
              return done("Dropped stash@{0}.", args.repoPath).then((r) => ({ ...r, stash: cfg.stashDrop }));
            case "changes_delete_branch":
              return done(`Deleted ${args.name}.`, args.repoPath).then((r) => ({ ...r, tree: cfg.branchDelete }));
            case "changes_commit":
              return done("commit ok", args.repoPath).then((r) =>
                args.repoPath === "/repos/gitswitch"
                  ? r
                  : {
                      ...r,
                      commit: {
                        hash: "9f1e2d3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        short: "9f1e2d3",
                        subject: args.message,
                        author_name: "Me",
                        author_email: "me@personal.test",
                        signature: "",
                        signed: false,
                        amended: !!args.amend,
                        files_changed: 1,
                        insertions: 5,
                        deletions: 1,
                      },
                    }
              );
            case "changes_submodules":
              return Promise.resolve(cfg.submodules);
            case "workspace_info":
              return Promise.resolve(cfg.workspace);
            case "workspace_set_offset":
              if (cfg.offsetRefusal) return Promise.reject(`Configuration error: ${cfg.offsetRefusal}`);
              return Promise.resolve({ ...cfg.workspace, offset: args.offset });
            case "ports_list":
              return Promise.resolve(cfg.ports);
            case "changes_lfs_status":
              return Promise.resolve(cfg.lfs);
            case "changes_lfs_files":
              return Promise.resolve(cfg.lfsListing);
            case "changes_lfs_progress":
              return Promise.resolve(cfg.lfsProgress);
            case "console_entries":
              // Answer once, the way the real log does: a poll asks for what is
              // newer than the highest id it has seen.
              return Promise.resolve((args?.sinceId ?? 0) > 0 ? [] : cfg.consoleEntries);
            case "console_clear":
              return Promise.resolve(null);
            case "path_exists":
              return Promise.resolve(!cfg.gonePaths.includes(args?.path));
            case "terminal_open":
              if (cfg.gonePaths.includes(args?.repoPath)) return Promise.reject(`Command execution error: Not found: ${args.repoPath} no longer exists on disk`);
              if (cfg.terminalFails.includes(args?.repoPath)) return Promise.reject("Command execution error: Could not open a terminal: no pty");
              // The session id arrives when the shell has started — at once, or late.
              return cfg.terminalOpenDelayMs
                ? new Promise((res) => setTimeout(() => res("test-session"), cfg.terminalOpenDelayMs))
                : Promise.resolve("test-session");
            case "terminal_write":
            case "terminal_resize":
            case "terminal_close":
              return Promise.resolve(null);
            case "install_commit_guard":
              return Promise.resolve("Guard installed");
            case "uninstall_commit_guard":
              return Promise.resolve("Guard removed.");
            case "changes_file_diff":
              return Promise.resolve(args.path === "logo.png" ? diffBinary : cfg.diffText ?? diffText);
            case "changes_pull":
              if (cfg.hang) return new Promise(() => {}); // never settles
              if (cfg.pullResult) return Promise.resolve({ ...cfg.pullResult, status: window.__SCENARIO__ });
              return done("Done.");
            case "changes_push":
              if (cfg.hang) return new Promise(() => {}); // never settles
              // The refusal is the repository's own; a submodule pushed first succeeds.
              if (cfg.pushResult && args.repoPath === window.__SCENARIO__.path) return Promise.resolve({ ...cfg.pushResult, status: window.__SCENARIO__ });
              // A submodule's own push answering with its own refusal.
              if (cfg.subPushResult && args.repoPath !== window.__SCENARIO__.path) return Promise.resolve({ ...cfg.subPushResult, status: statusFor(args.repoPath) });
              return done("Done.");
            case "changes_submodule_update":
            case "changes_submodule_align":
            case "changes_lfs_pull":
            case "changes_lfs_pull_paths":
              if (cfg.hang) return new Promise(() => {}); // never settles
              return done("Done.");
            case "changes_sync_plan": {
              window.__PLANNED__ = (window.__PLANNED__ ?? 0) + 1;
              const later = cfg.syncPlanSecond && window.__PLANNED__ > 1 ? cfg.syncPlanSecond : cfg.syncPlan;
              const base = args.stash === false ? cfg.syncPlanNoStash : (window.__RECORDED__ && cfg.syncPlanAfterRemedy) || later;
              // Tagged with the HEAD it was made for, as the backend does.
              return Promise.resolve({ ...base, head_oid: window.__SCENARIO__.head_oid });
            }
            case "changes_sync_run":
              if (cfg.hang) return new Promise(() => {}); // never settles
              if (cfg.syncRunHold) {
                return new Promise((res) => {
                  window.__RELEASE_RUN__ = (r) => res({ ...r, status: window.__SCENARIO__ });
                });
              }
              if (cfg.syncRunResult) return Promise.resolve({ ...cfg.syncRunResult, status: window.__SCENARIO__ });
              return Promise.resolve({
                ok: true,
                headline: "Updated: 2 commits of yours on top of 3 new commits from origin/main.",
                detail: "Your uncommitted changes were stashed and re-applied.",
                status: window.__SCENARIO__,
                sync: cfg.syncOutcome,
              });
            case "changes_sync_continue":
              if (cfg.syncContinueRefusal && !args.allowMarkers)
                return Promise.resolve({ ok: false, headline: cfg.syncContinueRefusal.message, detail: "", refusal: cfg.syncContinueRefusal, status: statusFor(args.repoPath) });
              return done("Updated: 3 commits of yours on top of 3 new commits from origin/main.");
            case "changes_sync_abort":
              return done("Update aborted.");
            case "changes_open_path":
              return Promise.resolve(null);
            case "changes_conflict_view":
              return Promise.resolve(cfg.conflictView);
            case "changes_record_pointers": {
              // Recording is a commit: HEAD moves, as it does for real. The
              // detail is the commit message, naming the paths it recorded.
              window.__SCENARIO__ = { ...window.__SCENARIO__, head_oid: "bbbb2222", ahead: (window.__SCENARIO__.ahead || 0) + 1 };
              window.__RECORDED__ = true;
              const recorded = args.paths ?? ["B"];
              return done(`Recorded ${recorded.length} submodule pointer(s).`).then((r) => ({ ...r, detail: `Record submodule pointers: ${recorded.join(", ")}` }));
            }
            case "changes_stage":
              if (cfg.stageRefusal && !args.allowMarkers)
                return Promise.resolve({ ok: false, headline: cfg.stageRefusal.message, detail: "", refusal: cfg.stageRefusal, status: statusFor(args.repoPath) });
            // falls through
            case "changes_continue":
              if (cfg.continueRefusal && !args.allowMarkers)
                return Promise.resolve({ ok: false, headline: cfg.continueRefusal.message, detail: "", refusal: cfg.continueRefusal, status: statusFor(args.repoPath) });
              return done("continue ok", args.repoPath);
            case "changes_unstage":
            case "changes_discard":
            case "changes_abort":
            case "changes_switch_branch":
            case "changes_rename_branch":
            case "changes_undo_commit":
            case "changes_resolve_side":
            case "changes_discard_all":
            case "changes_stash_push":
            case "changes_stash_apply":
            case "changes_stash_restore_file":
              return done(cmd.replace("changes_", "") + " ok", args.repoPath);
            case "changes_set_push_mode": {
              const cur = window.__SCENARIO__.push;
              if (cfg.lockOutcome !== "applied") {
                const messages = {
                  cancelled: "You cancelled the administrator prompt. Nothing changed.",
                  denied: "The administrator password was not accepted. Nothing changed.",
                  failed: "The lock helper failed (exit 4): disk full",
                  "manual-required": "No administrator prompt is available here. Run this command in a terminal as an administrator, then choose \"I ran it\".",
                };
                return Promise.resolve({
                  outcome: cfg.lockOutcome,
                  message: messages[cfg.lockOutcome] ?? "…",
                  command: cfg.lockOutcome === "manual-required" ? "sudo '/Library/PrivilegedHelperTools/com.gitswitch.lock-helper' '/Users/me/Library/Application Support/com.gitswitch.app/lock-jobs/n1.json' --sha256 abcdef" : null,
                  job_nonce: "n1",
                  state: cfg.lockOutcome === "manual-required" ? null : cur,
                });
              }
              const locked = args.mode === "locked";
              const guard = args.mode === "guardrail";
              const state = {
                ...cur,
                repo_blocked: locked || guard,
                blocked: locked || guard || cur.profile_blocked,
                reason: locked
                  ? "Locked for this repository. Unlocking needs the administrator password."
                  : guard
                    ? "Blocked for this repository (guard-rail)."
                    : "Pushing is allowed from this folder.",
                lock: {
                  ...cur.lock,
                  locked,
                  registry: locked ? "ok" : cur.lock.registry,
                  system_include: locked ? "ok" : "",
                  stanza: locked ? "ok" : "",
                  system_rewrite_measured: locked,
                  helper: locked ? "ok" : cur.lock.helper,
                  helper_installed: locked || cur.lock.helper_installed,
                  remote_helper: locked ? "ok" : cur.lock.remote_helper,
                  mirrors: "ok",
                  drift: [],
                  needs_elevation: false,
                  locked_at: locked ? "2026-09-23T09:15:00Z" : null,
                },
              };
              window.__PUSH_NOW__ = state;
              return Promise.resolve({
                outcome: "applied",
                message: locked ? "Locked 1 repository. Lock helper installed." : guard ? "Pushes are blocked (guard-rail)." : locked === false && cur.lock.locked ? "Unlocked 1 repository." : "Pushes are allowed again.",
                command: null,
                job_nonce: null,
                state,
              });
            }
            case "changes_repair_push_lock":
              if (cfg.repairLockResult) return Promise.resolve({ command: null, job_nonce: null, state: window.__SCENARIO__.push, ...cfg.repairLockResult });
              return Promise.resolve({
                outcome: "applied",
                message: "Everything is in place.",
                command: null,
                job_nonce: null,
                state: { ...window.__SCENARIO__.push, lock: { ...window.__SCENARIO__.push.lock, helper: "ok", remote_helper: "ok", drift: [], needs_elevation: false } },
              });
            case "push_lock_finish_manual":
              return Promise.resolve({ outcome: "applied", message: "Unlocked 1 repository.", command: null, job_nonce: null, changed: [], errors: [], bootstrapped: false });
            case "changes_push_coverage":
              return Promise.resolve(cfg.pushCoverage ?? { mode: "allowed", submodules: [], uncovered: [], needs_repair: false, summary: "" });
            case "changes_push_state":
              // A mode changed in this page is what a later read sees, as in the app.
              if (window.__PUSH_NOW__) return Promise.resolve(window.__PUSH_NOW__);
              if (cfg.pushState) return Promise.resolve({ ...window.__SCENARIO__.push, ...cfg.pushState, lock: { ...window.__SCENARIO__.push.lock, ...(cfg.pushState.lock || {}) } });
              return Promise.resolve({ ...window.__SCENARIO__.push, lock: { ...window.__SCENARIO__.push.lock, locked: false }, blocked: false, repo_blocked: false, reason: "Pushing is allowed from this folder." });
            case "changes_repair_push_block":
              return Promise.resolve({ ...window.__SCENARIO__.push, needs_repair: false });
            case "history_fetch":
              return Promise.resolve({ ok: true, message: "fetched" });
            case "doctor_check_environment":
            case "doctor_check_profiles":
            case "doctor_check_repos":
            case "doctor_check_keys":
            case "doctor_check_push_locks":
              return Promise.resolve([]);
            case "doctor_check_repo_access":
              return Promise.resolve([{ id: "remote-not-reachable:/work/app", severity: "warning", title: "@octo cannot see acme/app", detail: "GitHub answered for @octo: not found, with the repo scope.", fix: null, fix_label: null }]);
            // Find repositories
            case "find_unlisted_repos": {
              const empty = { home: "/Users/me", roots: ["/Users/me"], repos: [], already_listed: 0, truncated: [], unreadable: [], rejected: [], linked: [], cancelled: false };
              const result = window.__FOUND__ ?? (cfg.find && cfg.find.result) ?? empty;
              if (cfg.find && cfg.find.hold) return new Promise((res) => { window.__RELEASE_FIND__ = () => res(result); });
              return Promise.resolve(result);
            }
            case "find_repos_progress":
              return Promise.resolve({ phase: "looking", folders: 1536, found: 0, checked: 0, total: 0, place: "/Users/me/code" });
            case "find_repos_cancel":
              return Promise.resolve(null);
            case "get_profiles":
              return Promise.resolve(cfg.find ? cfg.find.profiles ?? [] : null);
            case "resolve_key_account":
              return Promise.resolve(((cfg.find && cfg.find.keyLogins) || {})[args.keyPath] ?? null);
            case "repo_access_matrix":
              return Promise.resolve((cfg.find && cfg.find.matrix) || { accounts: [], problems: [], access: {}, cancelled: false });
            case "local_clone_index":
              return Promise.resolve({ clones: {}, owner_profiles: (cfg.find && cfg.find.hints) || {}, org_ssh_users: {} });
            case "profile_folders_add":
              return Promise.resolve({ added: args.changes.map((c) => ({ ...c, guard_was: null })), failed: [] });
            case "profile_folders_remove":
              return Promise.resolve(args.changes.map((c) => ({ profile_id: c.profile_id, path: c.path })));
            case "plugin:dialog|open":
              return Promise.resolve((cfg.find && cfg.find.pick) || null);
            // GitHub: pull requests, reviews, merging, collaborators
            case "github_repo_context":
              return cfg.gh ? Promise.resolve(cfg.gh.context) : Promise.reject("Command execution error: no GitHub here");
            case "repo_accounts":
              return Promise.resolve(cfg.gh ? cfg.gh.accounts ?? [] : []);
            case "github_pulls": {
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              const all = gh.pulls
                .filter((p) => p.repo === `${args.owner}/${args.name}`)
                .filter((p) => !(gh.merged && p.number === gh.detail.number && args.state === "open"))
                .map((p) => (gh.merged && p.number === gh.detail.number ? { ...p, state: "merged" } : p));
              const want = all.filter((p) => args.state === "all" || p.state === args.state);
              const per = 2;
              const page = args.page || 1;
              return Promise.resolve({ items: want.slice((page - 1) * per, page * per), next_page: page * per < want.length ? page + 1 : null, total: null });
            }
            case "github_pull_repos":
              return Promise.resolve(cfg.gh.pullRepos ?? { login: "me-home", repos: [], truncated: false, notes: [] });
            case "github_pulls_search": {
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              return Promise.resolve({ items: gh.search.filter((p) => args.state === "all" || p.state === args.state).filter((p) => !args.repo || p.repo === args.repo), next_page: null, total: gh.search.length });
            }
            case "github_pull": {
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              const d = { ...gh.detail, ...(gh.details?.[args.number] ?? {}), number: args.number, repo: `${args.owner}/${args.name}` };
              return Promise.resolve(gh.merged && args.number === gh.detail.number ? { ...d, state: "merged", merged_by: "work-me", merged_at: new Date().toISOString() } : d);
            }
            case "github_pull_files":
              return Promise.resolve(cfg.gh.files);
            case "github_pull_commits":
              return Promise.resolve(cfg.gh.commits);
            case "github_pull_review":
              if (cfg.gh.reviewRefuse && args.event === "APPROVE") return Promise.reject(`Command execution error: ${cfg.gh.reviewRefuse}`);
              return Promise.resolve({ login: "work-me", state: args.event === "APPROVE" ? "APPROVED" : args.event === "COMMENT" ? "COMMENTED" : "CHANGES_REQUESTED", submitted_at: null });
            case "github_pull_merge": {
              if (cfg.gh.mergeRefuse) return Promise.reject(`Command execution error: ${cfg.gh.mergeRefuse}`);
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              gh.merged = true;
              return Promise.resolve({ merged: true, sha: "m3rg3d", message: "merged", branch_deleted: args.deleteBranch ? true : null, branch_note: args.deleteBranch ? `Branch ${gh.detail.head_ref} deleted.` : null });
            }
            case "github_collaborators": {
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              return Promise.resolve(gh.people);
            }
            case "github_collaborator_add": {
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              gh.people.invitations = [...(gh.people.invitations ?? []), { id: 90, login: args.username, role: args.permission, created_at: new Date().toISOString(), inviter: "work-me" }];
              return Promise.resolve({ status: "invited", message: `Invited ${args.username} — they get access once they accept the invitation GitHub sends them.`, invitation_id: 90 });
            }
            case "github_collaborator_remove": {
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              gh.people.collaborators = gh.people.collaborators.filter((c) => c.login !== args.username);
              return Promise.resolve(null);
            }
            case "github_invitation_cancel": {
              const gh = (window.__GH__ ??= JSON.parse(JSON.stringify(cfg.gh)));
              gh.people.invitations = gh.people.invitations.filter((i) => i.id !== args.id);
              return Promise.resolve(null);
            }
            case "plugin:opener|open_url":
              return Promise.resolve(null);
            default:
              return Promise.resolve(null);
          }
        },
      };
    }
  );
  if (opts.bridgeOnly) {
    return `(${bridge.toString()})(${[REPOS, scenario, DIFF_TEXT, DIFF_BINARY, cfg].map((a) => JSON.stringify(a)).join(",")});`;
  }
  const page = await browser.newPage();
  await page.setViewport({ width: cfg.width, height: 900 });
  await page.evaluateOnNewDocument(bridge, REPOS, scenario, DIFF_TEXT, DIFF_BINARY, cfg);
  await page.goto(`${server.url}/repo/changes`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 5000 });
  return page;
}

const text = (page) => page.evaluate(() => document.body.innerText);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Find a button by its visible label: an exact match wins, else the first one starting with it. */
async function button(page, label) {
  let prefix = null;
  for (const h of await page.$$("button")) {
    const b = (await h.evaluate((el) => el.innerText)).trim();
    if (b === label) return h;
    if (!prefix && b.startsWith(label)) prefix = h;
  }
  return prefix;
}

/** Click a button inside the first <li> whose text mentions `rowText` and that has the button. */
async function clickInRow(page, rowText, pick) {
  return page.evaluate(
    (rowText, pick) => {
      const rows = [...document.querySelectorAll("li")].filter((r) => r.innerText.includes(rowText));
      for (const row of rows) {
        const buttons = [...row.querySelectorAll("button")];
        const b =
          pick.title !== undefined
            ? buttons.find((x) => x.title === pick.title)
            : pick.text !== undefined
              ? buttons.find((x) => x.innerText.trim() === pick.text)
              : buttons[pick.index];
        if (b) {
          b.click();
          return true;
        }
      }
      return false;
    },
    rowText,
    pick
  );
}

/** A button's disabled state and tooltip, by its visible label. */
async function buttonState(page, label) {
  const h = await button(page, label);
  if (!h) return null;
  return h.evaluate((b) => ({ disabled: b.disabled, title: b.title, text: b.innerText.trim() }));
}

/** The Repositories page with its own mock bridge: two accounts, page-by-page listings. */
async function openReposPage(browser, server, opts = {}) {
  const cfg = {
    firstDelayBob: opts.firstDelayBob ?? 700,
    restDelay: opts.restDelay ?? 400,
    /** What local_clone_index answers: by default nothing is cloned. */
    local: opts.local ?? { clones: {}, owner_profiles: {}, org_ssh_users: {} },
  };
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    localStorage.setItem("gitswitch:repos.account", JSON.stringify("gh:alice"));
    const repo = (owner, i) => ({
      full_name: `${owner}/${owner[0]}-${i}`, owner, name: `${owner[0]}-${i}`, owner_is_org: false, description: null,
      private: i % 2 === 0, archived: false, fork: false, default_branch: "main", clone_url: `git@github.com:${owner}/${owner[0]}-${i}.git`,
      pushed_at: new Date(Date.now() - i * 3600e3).toISOString(), permission: "write", local_path: null, suggested_profile_id: null,
    });
    const listing = (login, repos, truncated = false) => ({ account: `gh:${login}`, login, suggested_profile_id: null, repos, truncated, sso_hidden_orgs: 0 });
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    window.__TAURI_INTERNALS__ = {
      invoke: async (cmd, args) => {
        window.__CALLS__.push({ cmd, args });
        switch (cmd) {
          case "repo_accounts":
            return [{ key: "gh:alice", label: "alice", source: "gh" }, { key: "gh:bob", label: "bob", source: "gh" }];
          case "get_profiles":
            return [];
          case "local_clone_index":
            return cfg.local;
          case "path_exists":
            return true;
          case "list_remote_repos_page": {
            const login = args.account === "gh:alice" ? "alice" : "bob";
            const total = 30;
            if (args.perPage === 10) {
              if (login === "bob") await wait(cfg.firstDelayBob);
              return { listing: listing(login, Array.from({ length: 10 }, (_, i) => repo(login, i + 1))), page: 1, per_page: 10, has_more: true };
            }
            await wait(cfg.restDelay);
            const start = (args.page - 1) * 100;
            const repos = Array.from({ length: Math.max(0, Math.min(100, total - start)) }, (_, i) => repo(login, start + i + 1));
            return { listing: listing(login, repos), page: args.page, per_page: 100, has_more: start + 100 < total };
          }
          default:
            return null;
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}/repos`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("Repositories"), { timeout: 5000 });
  return page;
}

/** The Clone page with its own mock bridge: it asks different questions than Changes. */
async function openClonePage(browser, server, opts = {}) {
  const cfg = {
    lfsTool: opts.lfsTool ?? { installed: true, version: "git-lfs/3.5.1 (GitHub; darwin arm64)", install_hint: "brew install git-lfs" },
    cloneResult: opts.cloneResult ?? null,
    mode: opts.mode ?? "full",
    /** "hang" keeps the clone running so the progress banner can be looked at. */
    cloneHang: opts.cloneHang ?? false,
    cloneError: opts.cloneError ?? null,
    cloneProgress: opts.cloneProgress ?? null,
    cloneSize: opts.cloneSize ?? { repo: "me/big-data", kb: 1258291 },
    profiles: opts.profiles ?? [],
    url: opts.url ?? "git@github.com:me/big-data.git",
    submodules: opts.submodules ?? null,
  };
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    localStorage.setItem("gitswitch:sparse.url", JSON.stringify(cfg.url));
    if (cfg.submodules !== null) localStorage.setItem("gitswitch:clone.submodules", JSON.stringify(cfg.submodules));
    localStorage.setItem("gitswitch:sparse.parentDir", JSON.stringify("/repos"));
    localStorage.setItem("gitswitch:clone.mode", JSON.stringify(cfg.mode));
    window.__TAURI_INTERNALS__ = {
      invoke: (cmd, args) => {
        window.__CALLS__.push({ cmd, args });
        switch (cmd) {
          case "get_profiles":
            return Promise.resolve(cfg.profiles);
          case "lfs_available":
            return Promise.resolve(cfg.lfsTool);
          case "clone_destination_status":
            return Promise.resolve({ path: "/repos/big-data", exists: false, is_repo: false, origin: null, same_repo: false, same_url: false, origin_is_https: false, incomplete: false });
          case "path_exists":
            return Promise.resolve(true);
          case "full_clone":
          case "sparse_clone":
            return cfg.cloneHang ? new Promise(() => {}) : cfg.cloneError ? Promise.reject(cfg.cloneError) : Promise.resolve(cfg.cloneResult);
          case "clone_size_estimate":
            return Promise.resolve(cfg.cloneSize);
          case "clone_progress":
            return Promise.resolve(cfg.cloneProgress);
          case "local_clone_index":
            return Promise.resolve({ by_path: {}, by_repo: {} });
          default:
            return Promise.resolve(null);
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}/sparse`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("Clone"), { timeout: 5000 });
  return page;
}

/**
 * The GitHub sign-in page against a scripted backend. Each device-flow call
 * answers from a list, in order: {resolve: value}, {reject: "text"} (sent the
 * way an AppError is: "Command execution error: text"), or "hold", which parks
 * the answer on window.__HELD__[cmd] for the test to release. An exhausted poll
 * list answers authorization_pending. With keepStorage, what the page stored
 * survives a reload or another page of the app.
 */
async function openAuthPage(browser, server, opts = {}) {
  const cfg = {
    ghAccounts: opts.ghAccounts ?? [],
    /** The login gh itself uses; null → gh's listing says nothing about it (the page shows the plain list). */
    ghActive: opts.ghActive ?? null,
    switchError: opts.switchError ?? null,
    code: { device_code: "dc", user_code: "ABCD-1234", verification_uri: "https://github.com/login/device", expires_in: opts.expires ?? 120, interval: 1 },
    codeHold: opts.codeHold ?? false,
    polls: opts.polls ?? [],
    verifies: opts.verifies ?? [],
    logoutError: opts.logoutError ?? null,
    keepStorage: opts.keepStorage ?? false,
    /** Logins whose sign-in the backend keeps; keepError: what keeping one answers. */
    kept: opts.kept ?? [],
    keepError: opts.keepError ?? null,
  };
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    if (!cfg.keepStorage || !sessionStorage.getItem("__seeded__")) {
      localStorage.clear();
      sessionStorage.setItem("__seeded__", "1");
    }
    let accounts = [...cfg.ghAccounts];
    let active = cfg.ghActive;
    let polls = 0;
    let verifies = 0;
    window.__HELD__ = {};
    const reply = (cmd, entry, fallback) => {
      if (entry === "hold") return new Promise((res, rej) => { window.__HELD__[cmd] = { res, rej }; });
      if (entry && entry.reject !== undefined) return Promise.reject(`Command execution error: ${entry.reject}`);
      if (entry && entry.resolve !== undefined) return Promise.resolve(entry.resolve);
      return fallback();
    };
    window.__TAURI_INTERNALS__ = {
      invoke: (cmd, args) => {
        window.__CALLS__.push({ cmd, args, at: Date.now() });
        switch (cmd) {
          case "gh_list_accounts":
            return accounts.length ? Promise.resolve(accounts) : Promise.reject("Command execution error: No gh accounts found. Run `gh auth login` first.");
          case "gh_logout":
            if (cfg.logoutError) return Promise.reject(cfg.logoutError);
            accounts = accounts.filter((a) => a !== args.account);
            return Promise.resolve(`Removed ${args.account} from the GitHub CLI`);
          case "gh_auth_accounts":
            if (active === null) return Promise.resolve([]);
            return Promise.resolve(accounts.map((login) => ({ host: "github.com", login, active: login === active, scopes: null })));
          case "gh_auth_switch":
            if (cfg.switchError) return Promise.reject(cfg.switchError);
            active = args.login;
            return Promise.resolve(`${args.login} is now the GitHub CLI's active account on ${args.host}`);
          case "github_device_code":
            return cfg.codeHold
              ? new Promise((res, rej) => { window.__HELD__.code = { res: () => res(cfg.code), rej }; })
              : Promise.resolve(cfg.code);
          case "github_poll_token":
            return reply(cmd, cfg.polls[polls++], () => Promise.reject("Command execution error: authorization_pending"));
          case "verify_github_token":
            return reply(cmd, cfg.verifies[verifies++], () => Promise.reject("Command execution error: nothing scripted"));
          case "github_signins":
            return Promise.resolve(cfg.kept.map((login, i) => ({ login, id: i + 1 })));
          case "github_signin_keep":
            return cfg.keepError ? Promise.reject(cfg.keepError) : Promise.resolve(null);
          case "github_signin_forget":
            return Promise.resolve(null);
          case "get_profiles":
          case "list_ssh_keys":
            return Promise.resolve([]);
          default:
            return Promise.resolve(null);
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}/github`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("Sign in with GitHub"), { timeout: 5000 });
  return page;
}

/** The SSH Keys page against a mocked backend: one key from an earlier visit, two gh logins. */
async function openSshPage(browser, server, opts = {}) {
  const cfg = {
    register: opts.register ?? { resolve: "Registered on alice" },
    noClipboard: opts.noClipboard ?? false,
    /** Sign-ins kept here: they register keys too. */
    kept: opts.kept ?? [],
  };
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    if (cfg.noClipboard) {
      Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
      document.execCommand = () => false;
    }
    window.__TAURI_INTERNALS__ = {
      invoke: (cmd, args) => {
        window.__CALLS__.push({ cmd, args });
        switch (cmd) {
          case "list_ssh_keys":
            return Promise.resolve(["/home/me/.ssh/id_ed25519_gitswitch_work"]);
          case "gh_list_accounts":
            return Promise.resolve(["alice", "bob"]);
          case "github_signins":
            return Promise.resolve(cfg.kept.map((login, i) => ({ login, id: i + 1 })));
          case "get_public_key":
            return Promise.resolve("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGitSwitchTestKey me@example.test");
          case "gh_register_ssh_key":
            return cfg.register.reject ? Promise.reject(`Command execution error: ${cfg.register.reject}`) : Promise.resolve(cfg.register.resolve);
          case "resolve_key_account":
            return Promise.resolve("alice");
          case "generate_ssh_key":
            return Promise.resolve(["/home/me/.ssh/id_ed25519_new", "/home/me/.ssh/id_ed25519_new.pub"]);
          default:
            return Promise.resolve(null);
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}/ssh`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("Existing SSH Keys"), { timeout: 5000 });
  return page;
}

/** The Ports page against a mocked backend: a listing of rows, and what Stop answers. */
async function openPortsPage(browser, server, opts = {}) {
  const cfg = {
    rows: opts.rows ?? PORT_ROWS,
    killResult: opts.killResult ?? null,
    latency: opts.latency ?? 0,
    storage: opts.storage ?? {},
    /** The address's query, as a repository's workspace line opens the page ("?clone=…"). */
    query: opts.query ?? "",
  };
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    for (const [k, v] of Object.entries(cfg.storage)) localStorage.setItem(`gitswitch:${k}`, JSON.stringify(v));
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    window.__TAURI_INTERNALS__ = {
      invoke: async (cmd, args) => {
        window.__CALLS__.push({ cmd, args, at: Date.now() });
        switch (cmd) {
          case "ports_list":
            if (cfg.latency) await wait(cfg.latency);
            return { os: "macos", docker: "ok", took_ms: 50, notes: ["Processes of other users (including root) are listed from netstat; GitSwitch cannot inspect their folder or stop them."], rows: cfg.rows };
          case "ports_kill":
            return cfg.killResult ?? { pid: args.pid, ended: true, how: "term", group: true, message: `Stopped ${args.name} (pid ${args.pid}).` };
          case "ports_container_stop":
            return { stopped: [args.id], message: "Stopped carpool-proxy-1." };
          default:
            return null;
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}/ports${cfg.query}`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("Every listener on this"), { timeout: 5000 });
  return page;
}

const PORT_ROWS = [
  { key: "tcp:18789:725", proto: "tcp", port: 18789, addresses: ["127.0.0.1", "::1"], scope: "loopback", pid: 725, name: "node", exe: "/opt/homebrew/bin/node", cwd: "/Users/me/Documents/Acme/gitswitch", started_at: "2026-10-11T01:00:00+05:30", user: "me", ours: true, workspace: "abc12345", clone: { path: "/Users/me/Documents/Acme/gitswitch", name: "gitswitch", profile_name: "Kirmada" }, container: null, killable: true, why_not: null },
  { key: "tcp:27017:720", proto: "tcp", port: 27017, addresses: ["127.0.0.1", "::1"], scope: "loopback", pid: 720, name: "mongod", exe: "/opt/homebrew/bin/mongod", cwd: "/opt/homebrew", started_at: "2026-10-11T00:13:40+05:30", user: "me", ours: false, workspace: null, clone: null, container: null, killable: true, why_not: null },
  { key: "tcp:8080:2842", proto: "tcp", port: 8080, addresses: ["*"], scope: "all", pid: 2842, name: "com.docker.backend", exe: null, cwd: null, started_at: null, user: "me", ours: false, workspace: null, clone: null, container: { id: "f50b929f94ba", name: "carpool-proxy-1", project: "carpool", service: "proxy", working_dir: "/Users/me/x", container_port: 80 }, killable: false, why_not: "Published by Docker — stop the container instead." },
  { key: "tcp:8021:1", proto: "tcp", port: 8021, addresses: ["127.0.0.1", "::1"], scope: "loopback", pid: 1, name: "launchd", exe: "/sbin/launchd", cwd: null, started_at: null, user: "root", ours: false, workspace: null, clone: null, container: null, killable: false, why_not: "Owned by root — needs an administrator: stop it from Terminal with sudo kill 1." },
  { key: "tcp:5000:999", proto: "tcp", port: 5000, addresses: ["*"], scope: "all", pid: 999, name: "python3", exe: "/usr/bin/python3", cwd: "/Users/me/Documents/releaseengineeringautomationandplatforminfrastructureteamnotifications/workspace/apps/web/server", started_at: null, user: "me", ours: false, workspace: null, clone: null, container: null, killable: true, why_not: null },
];

/**
 * Auto Assign against a mocked backend: a profile with a key, one with only a saved account.
 * opts: repos (what the scan finds), dirs ({profileId: folders}), hold (the scan answers only
 * when window.__RELEASE_SCAN__() is called), push ({"owner/name": [logins that can push]}).
 */
async function openAutoAssignPage(browser, server, opts = {}) {
  const page = await browser.newPage();
  await page.setViewport({ width: opts.width ?? 900, height: 900 });
  const cfg = {
    repos: opts.repos ?? [
      { path: "/w/widgets", remote_url: "git@github.com:acme/widgets.git", owner: "acme", name: "widgets" },
      { path: "/w/secret", remote_url: "git@github.com:acme/secret.git", owner: "acme", name: "secret" },
    ],
    dirs: opts.dirs ?? {},
    hold: opts.hold ?? false,
    push: opts.push ?? { "acme/widgets": ["acme-bot"] },
  };
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    localStorage.setItem("gitswitch:autoassign.root", JSON.stringify("/w"));
    const prof = (id, name, key, login, isDefault) => ({ id, name, git_name: name, git_email: `${id}@example.test`, ssh_key_path: key, is_default: isDefault, allow_push: true, signing_enabled: false, directories: cfg.dirs[id] ?? [], remote_patterns: [], github_login: login, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" });
    window.__TAURI_INTERNALS__ = {
      invoke: (cmd, args) => {
        window.__CALLS__.push({ cmd, args });
        switch (cmd) {
          case "get_profiles":
            return Promise.resolve([prof("p1", "Personal", "/k", null, true), prof("pk", "Bot", null, "acme-bot", false)]);
          case "scan_repos":
            return Promise.resolve(cfg.repos);
          case "scan_repos_tracked": {
            const done = { root: args.root, repos: cfg.repos, folders: 42, cancelled: false };
            if (!cfg.hold) return Promise.resolve(done);
            return new Promise((res) => {
              window.__RELEASE_SCAN__ = (v) => res(v ?? done);
            });
          }
          case "find_repos_progress":
            return Promise.resolve({ phase: "looking", folders: 1234, found: 1, checked: 0, total: 0, place: "/w/client/deep/folder" });
          case "find_repos_cancel":
            return Promise.resolve(null);
          case "resolve_key_account":
            return Promise.resolve("me-personal");
          case "gh_list_accounts":
            return Promise.resolve(["acme-bot", "me-personal"]);
          case "gh_get_token":
            return Promise.resolve(`t-${args.account}`);
          case "verify_github_token":
            return Promise.resolve({ id: 1, login: String(args.token).slice(2), name: null, email: null, avatar_url: "" });
          case "check_repo_access":
            return Promise.resolve((cfg.push[`${args.owner}/${args.repo}`] ?? []).includes(String(args.token).slice(2)) ? { admin: false, push: true, pull: true } : null);
          case "update_profile":
            return Promise.resolve({});
          default:
            return Promise.resolve(null);
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}/auto-assign`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("Auto Assign"), { timeout: 5000 });
  return page;
}

/** The Uninstall tab (or Settings, for GitSwitch's own uninstall) against a mocked backend. */
async function openUninstallPage(browser, server, opts = {}) {
  const page = await browser.newPage();
  await page.setViewport({ width: opts.width ?? 1100, height: 900 });
  const app = (id, name, extra = {}) => ({ id, name, version: "1.0", publisher: null, location: `/Applications/${name}.app`, source: "app", size: null, date: null, notes: [], blocked: null, is_self: false, ...extra });
  const cfg = {
    platform: opts.platform ?? "macos",
    apps: opts.apps ?? [
      app("mac:/Applications/Foo Writer.app", "Foo Writer", { size: 5000000 }),
      app("mac:/Applications/Bar Tool.app", "Bar Tool", { size: 9000000 }),
      app("mac:/Applications/Xcode.app", "Xcode", { blocked: "GitSwitch's git comes from it (xcode-select points into it)." }),
      app("mac:/Applications/GitSwitch.app", "GitSwitch", { is_self: true }),
    ],
    packages: opts.packages ?? [],
    sessions: opts.sessions ?? [],
    prepared: opts.prepared ?? {},
    uninstall: opts.uninstall ?? { state: "gone", message: null },
    status: opts.status ?? [],
    checkGone: opts.checkGone ?? { state: "gone", message: null },
    scan: opts.scan ?? {
      scan: "scan-1",
      notes: ["macOS still keeps the installer's record \"com.acme.pkg\". To forget it: sudo pkgutil --forget com.acme.pkg"],
      items: [
        { id: "l0", path: "/Users/me/Library/Caches/com.acme.foowriter", place: "~/Library/Caches", kind: "folder", size: 3000, size_complete: true, confidence: "high", reason: "named after its id com.acme.foowriter", admin: false, command: null },
        { id: "l1", path: "/Users/me/Library/Preferences/com.acme.foowriter.plist", place: "~/Library/Preferences", kind: "file", size: 10, size_complete: true, confidence: "high", reason: "com.acme.foowriter.plist", admin: false, command: null },
        { id: "l2", path: "/Users/me/Library/Application Support/Foo Writer", place: "~/Library/Application Support", kind: "folder", size: 100, size_complete: false, confidence: "medium", reason: "named like the app (Foo Writer)", admin: false, command: null },
        { id: "l3", path: "/Library/Application Support/com.acme.foowriter", place: "/Library/Application Support", kind: "folder", size: null, size_complete: false, confidence: "high", reason: "named after its id com.acme.foowriter", admin: true, command: "sudo rm -rf '/Library/Application Support/com.acme.foowriter'" },
      ],
    },
    plan: opts.plan ?? null,
    selfPrepare: opts.selfPrepare ?? { outcome: "done", message: "GitSwitch's changes are taken back.", command: null, done: ["Removed GitSwitch's block from ~/.gitconfig (a backup is beside it)"], problems: [] },
  };
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    const preparedFor = (id) => {
      const a = [...cfg.apps, ...cfg.packages].find((x) => x.id === id);
      return { session: "sess-" + a.name.replace(/\W/g, ""), app: a, method: `${a.location} is moved to the Trash.`, command: null, facts: ["Bundle id: com.acme.thing"], also_removed: [], blocked: null, running: false, warnings: [], vendor_uninstaller: null, ...(cfg.prepared[id] ?? {}) };
    };
    window.__TAURI_INTERNALS__ = {
      transformCallback: () => 1,
      invoke: (cmd, args) => {
        window.__CALLS__.push({ cmd, args });
        switch (cmd) {
          case "apps_list":
            return Promise.resolve({ platform: cfg.platform, notes: [], apps: args.kind === "packages" ? cfg.packages : cfg.apps });
          case "apps_sessions":
            return Promise.resolve(cfg.sessions);
          case "apps_size":
            return Promise.resolve([1234, true]);
          case "apps_prepare":
            return Promise.resolve(preparedFor(args.id));
          case "apps_uninstall":
            return Promise.resolve(cfg.uninstall);
          case "apps_status":
            return Promise.resolve(cfg.status.length ? cfg.status.shift() : { state: "uninstalling", message: "The uninstaller is open — follow its steps." });
          case "apps_check_gone":
            return Promise.resolve(cfg.checkGone);
          case "apps_scan":
            return Promise.resolve(cfg.scan);
          case "apps_delete":
            return Promise.resolve(args.ids.map((id) => ({ id, outcome: "trashed", message: "Moved to the Trash." })));
          case "apps_forget":
          case "apps_open_uninstaller":
            return Promise.resolve(null);
          case "get_current_git_config":
            return Promise.resolve("");
          case "push_lock_helper_status":
            return Promise.resolve({ supported: false, locks: [], installed: false, helper: "missing" });
          case "uninstall_plan":
            return Promise.resolve(cfg.plan);
          case "uninstall_prepare":
            return Promise.resolve(cfg.selfPrepare);
          case "uninstall_finish":
            return Promise.resolve({ how: "/Applications/GitSwitch.app is moved to the Trash once GitSwitch has quit.", manual: null, log: "/tmp/gitswitch-uninstall.log" });
          default:
            return Promise.resolve(null);
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}${opts.path ?? "/uninstall"}`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes(location.pathname === "/settings" ? "Settings" : "Uninstall"), { timeout: 5000 });
  return page;
}

/** The profile form against a mocked backend: its profiles, gh logins and GitHub's answers. */
async function openProfilePage(browser, server, opts = {}) {
  const cfg = {
    path: opts.path ?? "/profile/new",
    profiles: opts.profiles ?? [],
    ghAccounts: opts.ghAccounts ?? [],
    user: opts.user ?? null,
    keyAccount: opts.keyAccount ?? null,
  };
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    window.__TAURI_INTERNALS__ = {
      invoke: (cmd, args) => {
        window.__CALLS__.push({ cmd, args });
        switch (cmd) {
          case "get_profiles":
            return Promise.resolve(cfg.profiles);
          case "list_ssh_keys":
            return Promise.resolve([]);
          case "gh_list_accounts":
            return Promise.resolve(cfg.ghAccounts);
          case "gh_get_token":
            return Promise.resolve("tok");
          case "verify_github_token":
            return Promise.resolve(cfg.user ?? { id: 1, login: args?.token ?? "someone", name: null, email: null, avatar_url: "" });
          case "resolve_key_account":
            return cfg.keyAccount ? Promise.resolve(cfg.keyAccount) : Promise.reject("the key could not be probed");
          case "create_profile":
          case "update_profile":
            return Promise.resolve({ ...(cfg.profiles[0] ?? {}), ...args });
          default:
            return Promise.resolve(null);
        }
      },
    };
  }, cfg);
  await page.goto(`${server.url}${cfg.path}`, { waitUntil: "networkidle0" });
  return page;
}

// webkit.sh loads the built app into the system's WebKit — the engine the app
// itself runs in on macOS — and needs this same bridge as a script to inject.
if (process.env.GITSWITCH_BRIDGE_OUT) {
  const src = await openPage(null, null, SCENARIOS.clean, {
    bridgeOnly: true,
    terminalOpenDelayMs: Number(process.env.GITSWITCH_TERMINAL_DELAY_MS ?? 0),
  });
  fs.writeFileSync(process.env.GITSWITCH_BRIDGE_OUT, src);
  process.exit(0);
}

const server = await serve();
const browser = await launch();
const open = (scenario, opts) => openPage(browser, server, scenario, opts);

try {
  // -----------------------------------------------------------------
  section("A clean, up-to-date repo");
  {
    const page = await open(SCENARIOS.clean);
    const s = await text(page);
    ok("says there is nothing to pull", s.includes("Nothing to pull"));
    ok("shows the branch and its upstream", s.includes("main") && s.includes("origin/main"));
    ok("commit is blocked with nothing staged", s.includes("Nothing is staged"));
    const commit = await button(page, "Commit");
    ok("and the Commit button is disabled", await commit.evaluate((b) => b.disabled));
    ok("staleness is shown in words", /fetched \d+ min ago|fetched just now/.test(s));
    ok("no LFS card when the repo doesn't use LFS", !s.includes("Git LFS"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("A repo with every kind of change");
  {
    const page = await open(SCENARIOS.dirty);
    const s = await text(page);
    ok("groups staged, not staged and untracked separately", s.includes("Staged") && s.includes("Not staged") && s.includes("Untracked"));
    ok("a file staged AND modified appears in both groups", (s.match(/src\/both\.ts/g) ?? []).length === 2);
    ok("line counts are shown", s.includes("+10") && s.includes("−2"));
    ok("a binary file is labelled instead of counted", s.includes("binary"));
    ok("filenames with spaces render", s.includes("new file.txt"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Pull: the mode is explained before it runs");
  {
    const page = await open(SCENARIOS.behindOnly);
    const s = await text(page);
    ok("behind only: says the branch just moves forward", s.includes("moves forward") && s.includes("4 new commits"));
    ok("and nothing of yours changes", s.includes("Nothing of yours changes"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged);
    let s = await text(page);
    ok("diverged: fast-forward explains why it can't", s.includes("Can't fast-forward") || s.includes("Not possible"));
    ok("the Pull button is disabled in that mode", await (await button(page, "Pull")).evaluate((b) => b.disabled));
    await (await button(page, "Merge")).click();
    s = await text(page);
    ok("merge: names the merge commit and hash stability", s.includes("merge commit") && s.includes("hashes stay the same"));
    ok("and the button now says Pull (merge)", s.includes("Pull (merge)"));
    await (await button(page, "Rebase")).click();
    s = await text(page);
    ok("rebase: warns the commits get new hashes", s.includes("new hashes"));
    ok("and Pull is enabled again", !(await (await button(page, "Pull")).evaluate((b) => b.disabled)));
    await page.close();
  }
  {
    const page = await open({ ...SCENARIOS.diverged, staged_count: 1, unstaged_count: 1 }, { pullMode: "rebase" });
    const s = await text(page);
    ok("rebase on a dirty tree explains the refusal", s.includes("clean working tree"));
    ok("and Pull is disabled", await (await button(page, "Pull")).evaluate((b) => b.disabled));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Push");
  {
    const page = await open(SCENARIOS.unpublished);
    const s = await text(page);
    ok("an unpublished branch offers Publish branch", s.includes("Publish branch"));
    ok("and says it isn't published yet", s.includes("not published yet"));
    await page.close();
  }
  const segment = (page, label) =>
    page.evaluate((l) => {
      const b = [...document.querySelectorAll('[role="radiogroup"][aria-label="Push access"] button')].find((x) => x.getAttribute("aria-label") === l);
      return b ? { checked: b.getAttribute("aria-checked") === "true", disabled: b.disabled, title: b.title } : null;
    }, label);
  const clickSegment = (page, label) =>
    page.evaluate((l) => {
      const b = [...document.querySelectorAll('[role="radiogroup"][aria-label="Push access"] button')].find((x) => x.getAttribute("aria-label") === l);
      if (!b) return false;
      b.click();
      return true;
    }, label);
  const openDetails = (page, label) =>
    page.evaluate((l) => {
      const d = [...document.querySelectorAll("summary")].find((x) => x.innerText.includes(l));
      if (!d) return false;
      d.click();
      return true;
    }, label);
  {
    // The block reaches into the submodules, and says so; one left out is named, with Repair.
    const full = { mode: "guardrail", submodules: [{ path: "trinity", blocked: true, locked: false, needs_repair: false, covered: true }, { path: "staging", blocked: true, locked: false, needs_repair: false, covered: true }], uncovered: [], needs_repair: false, summary: "Covers this repository and 2 submodules." };
    const page = await open(SCENARIOS.blocked, { pushCoverage: full });
    await page.waitForFunction(() => document.body.innerText.includes("Covers this repository"), { timeout: 4000 }).catch(() => null);
    ok("a blocked superproject says the block covers its submodules", (await text(page)).includes("Covers this repository and 2 submodules."));
    await page.close();
    const partial = { ...full, submodules: [full.submodules[0], { ...full.submodules[1], blocked: false, covered: false }], uncovered: ["staging"], needs_repair: true, summary: "Not covering 1 of 2 submodules: staging — a push from inside it is not stopped." };
    const page2 = await open(SCENARIOS.blocked, { pushCoverage: partial });
    await page2.waitForFunction(() => document.body.innerText.includes("Not covering"), { timeout: 4000 }).catch(() => null);
    const s2 = await text(page2);
    ok("  a submodule the block does not reach is named, with what that means", s2.includes("Not covering 1 of 2 submodules: staging") && s2.includes("not stopped"));
    const repair = await button(page2, "Repair — cover them too");
    ok("  and Repair is offered for it", repair !== null);
    await repair?.click();
    await page2.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_repair_push_block"), { timeout: 4000 }).catch(() => null);
    ok("  which re-applies the block to the repository and its submodules", await page2.evaluate(() => window.__CALLS__.some((c) => c.cmd === "changes_repair_push_block" && c.args.repoPath === "/repos/gitswitch")));
    await page2.close();
  }
  {
    // Locked, and a submodule's own settings lost part of the block: Repair
    // needs no password there, so it promises no prompt; afterwards the
    // coverage is read again, though this repository's own state is the same.
    const why = "it is locked, but its own settings lost part of the block; Repair puts it back without a password";
    const cov = { mode: "locked", submodules: [{ path: "stock", blocked: true, locked: true, needs_repair: false, covered: false, why, by_superproject: true, drift: ["mirror-rewrite"] }], uncovered: ["stock"], needs_repair: true, needs_elevation: false, summary: `Not covering 1 of 1 submodule: stock (${why}).` };
    const page = await open(SCENARIOS.locked, { pushCoverage: cov });
    await page.waitForFunction(() => document.body.innerText.includes("Not covering"), { timeout: 4000 }).catch(() => null);
    const labels = await page.evaluate(() => [...document.querySelectorAll("button")].map((b) => b.innerText.trim()));
    ok("a submodule whose own settings lost part of the lock's block is offered Repair, promising no prompt", labels.includes("Repair — cover them too") && !labels.some((l) => l.includes("cover them too (administrator prompt)")), JSON.stringify(labels.filter((l) => l.startsWith("Repair"))));
    const title = await page.evaluate(() => [...document.querySelectorAll("p")].find((p) => p.innerText.startsWith("Not covering"))?.title ?? "");
    ok("  and the line's hover says what was measured", title.includes("stock: mirror-rewrite"), title);
    const reads = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_push_coverage").length);
    const before = await reads();
    await (await button(page, "Repair — cover them too"))?.click();
    await page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "changes_push_coverage").length > n, { timeout: 4000 }, before).catch(() => null);
    ok("  after Repair the coverage is read again, though this repository's own state is unchanged", (await reads()) > before, `${before} -> ${await reads()}`);
    await page.close();
  }
  {
    // A submodule's lock layers drifted: Repair does need the administrator, and says so.
    const cov = { mode: "locked", submodules: [{ path: "stock", blocked: true, locked: true, needs_repair: false, covered: false, why: "its lock needs repair", by_superproject: true, drift: ["stanza-stale"] }], uncovered: ["stock"], needs_repair: true, needs_elevation: true, summary: "Not covering 1 of 1 submodule: stock (its lock needs repair)." };
    const page = await open(SCENARIOS.locked, { pushCoverage: cov });
    await page.waitForFunction(() => document.body.innerText.includes("Not covering"), { timeout: 4000 }).catch(() => null);
    ok("a submodule whose lock layers drifted is offered Repair with the administrator prompt named", await page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Repair — cover them too (administrator prompt)")));
    await page.close();
  }
  {
    // Repair that could not cover everything says so in amber, never a green "in place".
    const cov = { mode: "locked", submodules: [{ path: "stock", blocked: true, locked: true, needs_repair: false, covered: false, why: "its lock needs repair", by_superproject: true, drift: ["stanza-stale"] }], uncovered: ["stock"], needs_repair: true, needs_elevation: true, summary: "Not covering 1 of 1 submodule: stock (its lock needs repair)." };
    const message = "Repaired 1 repository. Not covering 1 of 1 submodule: stock (its lock needs repair).";
    const page = await open(SCENARIOS.locked, { pushCoverage: cov, repairLockResult: { outcome: "incomplete", message } });
    await page.waitForFunction(() => document.body.innerText.includes("Not covering"), { timeout: 4000 }).catch(() => null);
    await (await button(page, "Repair — cover them too"))?.click();
    await page.waitForFunction((m) => document.body.innerText.includes(m), { timeout: 4000 }, message).catch(() => null);
    const box = await page.evaluate((m) => [...document.querySelectorAll("div")].reverse().find((d) => d.innerText.trim().startsWith(m.slice(0, 20)) && d.className.includes("rounded-lg border"))?.className ?? "", message);
    ok("a Repair that left a submodule uncovered says so in amber, not as done", box.includes("amber") && !box.includes("emerald"), box);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.blocked);
    const s = await text(page);
    ok("a blocked repo says so on the button", s.includes("Push blocked"));
    ok("and the button is disabled", await (await button(page, "Push blocked")).evaluate((b) => b.disabled));
    ok("the reason names the repository", s.includes("Blocked for this repository"));
    ok("the Guard-rail segment is the one on", (await segment(page, "Guard-rail"))?.checked === true);
    ok("the rewrite state per remote is shown", s.includes("origin") && s.includes("rewritten"));
    await openDetails(page, "What this doesn't stop");
    const s2 = await text(page);
    ok("gaps are disclosed, including --no-verify", s2.includes("--no-verify"));
    ok("and it never claims to be a lock", s2.includes("guard-rail, not a lock"));
    ok("and names the env-var bypass", s2.includes("GIT_CONFIG_NOSYSTEM"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.profileBlocked);
    const s = await text(page);
    ok("a profile-level block names the profile", s.includes("Blocked by profile 'Work'"));
    ok("Allowed is disabled (the UI treats the profile block as binding)", (await segment(page, "Allowed"))?.disabled === true);
    ok("  and says to change it in the profile", ((await segment(page, "Allowed"))?.title ?? "").includes("profile"));
    ok("  but Locked is still offered", (await segment(page, "Locked"))?.disabled === false);
    ok("  and the Guard-rail can still be added for this repository", (await segment(page, "Guard-rail"))?.disabled === false);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.needsRepair);
    const s = await text(page);
    ok("a drifted block offers to re-apply", s.includes("isn't fully in place") && s.includes("Re-apply"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Push lock");
  {
    const page = await open(SCENARIOS.locked);
    const s = await text(page);
    ok("the push button says locked", s.includes("Push locked"));
    ok("the Locked segment is on", (await segment(page, "Locked"))?.checked === true);
    ok("the reason names the administrator password", s.includes("administrator password"));
    ok("every layer is shown as measured", s.includes("System-wide rule") && s.includes("git itself reports the rewrite at system scope"));
    ok("the helper line is shown", s.includes("Lock helper") && s.includes("checksum matches"));
    ok("the mirrors line is shown", s.includes("Repository mirrors"));
    ok("the lock time is shown", s.includes("Locked 2026-09-23"));
    ok("the guard-rail caveat is NOT shown under a lock", !s.includes("guard-rail, not a lock"));
    await openDetails(page, "What this doesn't stop");
    const s2 = await text(page);
    ok("the caveats name the real bypass", s2.includes("GIT_CONFIG_NOSYSTEM=1"));
    ok("  and what the lock gives you", s2.includes("administrator prompt"));
    const caveats = await page.evaluate(() => {
      const d = [...document.querySelectorAll("details")].find((x) => x.innerText.includes("What this doesn't stop"));
      return d ? [...d.querySelectorAll("li")].map((li) => li.innerText) : [];
    });
    ok("  and never point at GitHub (the user chose a local lock)", caveats.length > 0 && caveats.every((c) => !c.includes("GitHub")));
    await openDetails(page, "Recent events");
    const s3 = await text(page);
    ok("recent events are listed", s3.includes("locked") && s3.includes("Lock helper installed"));
    ok("  with the honest sentence about the user-side log", s3.includes("can be edited without a password"));
    ok("  naming the administrator-only record", s3.includes("/etc/gitswitch/audit.log"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lockedHealed);
    const s = await text(page);
    ok("healed mirrors are called out", s.includes("put them back just now"));
    await openDetails(page, "Recent events");
    const s2 = await text(page);
    ok("  and the event says what drifted", s2.includes("mirror-flag, mirror-hook"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lockedNeedsRepair);
    const s = await text(page);
    ok("a drifted admin layer offers Repair with the prompt", s.includes("Repair (administrator prompt)"));
    ok("  naming the drift codes", s.includes("helper-missing"));
    ok("  and the helper line says the lock still holds", s.includes("The lock still holds"));
    const clicked = await (await button(page, "Repair (administrator prompt)")).click().then(() => true);
    await sleep(300);
    const s2 = await text(page);
    ok("repairing reports the outcome", clicked && s2.includes("Everything is in place."));
    ok("  and the Repair banner is gone", !s2.includes("Repair (administrator prompt)"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lockUnsupported);
    const seg = await segment(page, "Locked");
    ok("on an unsupported platform Locked is disabled", seg?.disabled === true);
    ok("  with a reason in its tooltip", (seg?.title ?? "").includes("not available"));
    await page.close();
  }
  {
    // A guard-railed repository whose earlier lock reached a submodule that is
    // still locked: "Allowed" unlocks that submodule, so it asks first and
    // names it — the administrator prompt must never arrive unannounced.
    const cov = { mode: "guardrail", submodules: [
      { path: "trinity", blocked: true, locked: true, needs_repair: false, covered: true, why: "", by_superproject: true },
      { path: "staging", blocked: true, locked: true, needs_repair: false, covered: true, why: "", by_superproject: false },
    ], uncovered: [], needs_repair: false, summary: "Covers this repository and 2 submodules." };
    const page = await open(SCENARIOS.blocked, { pushCoverage: cov });
    await page.waitForFunction(() => document.body.innerText.includes("Covers this repository"), { timeout: 4000 }).catch(() => null);
    await clickSegment(page, "Allowed");
    await sleep(200);
    const s = await text(page);
    ok("Allowed on a repository whose lock reached a submodule asks first, naming that submodule and the prompt",
      s.includes("also unlocks the submodules this repository's lock reached (trinity)") && !s.includes("(trinity, staging)") && (await button(page, "Unlock (administrator prompt)")) !== null,
      s.slice(s.indexOf("Allowing pushes again"), s.indexOf("Allowing pushes again") + 220));
    ok("  and nothing has run yet", !(await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "changes_set_push_mode"))));
    await page.close();
  }
  {
    // Locking from Allowed: confirmation first, then the outcome.
    const page = await open(SCENARIOS.clean);
    ok("Allowed is on for a plain repo", (await segment(page, "Allowed"))?.checked === true);
    await clickSegment(page, "Locked");
    await sleep(150);
    const s = await text(page);
    ok("choosing Locked asks first, naming the repository and the prompt", s.includes("/repos/gitswitch") && s.includes("macOS will ask for an administrator name and password"));
    ok("  and explains the first-time helper install with its checksum", s.includes("installs its lock helper") && s.includes("ab12cd34ef56"));
    ok("  nothing has run yet", !(await text(page)).includes("Locked 1 repository"));
    await (await button(page, "Lock (administrator prompt)")).click();
    await sleep(300);
    const s2 = await text(page);
    ok("after the prompt the outcome is shown", s2.includes("Locked 1 repository."));
    ok("  and the segment moved to Locked", (await segment(page, "Locked"))?.checked === true);
    ok("  and the push button says locked", s2.includes("Push locked"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { lockOutcome: "cancelled" });
    await clickSegment(page, "Locked");
    await sleep(150);
    await (await button(page, "Lock (administrator prompt)")).click();
    await sleep(300);
    const s = await text(page);
    ok("a cancelled prompt is reported as cancelled", s.includes("Cancelled.") && s.includes("Nothing changed."));
    ok("  and Allowed is still on", (await segment(page, "Allowed"))?.checked === true);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { lockOutcome: "denied" });
    await clickSegment(page, "Locked");
    await sleep(150);
    await (await button(page, "Lock (administrator prompt)")).click();
    await sleep(300);
    const s = await text(page);
    ok("a refused password is reported as not allowed", s.includes("Not allowed.") && s.includes("was not accepted"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.locked, { lockOutcome: "manual-required" });
    await clickSegment(page, "Allowed");
    await sleep(150);
    const s0 = await text(page);
    ok("unlocking asks first and says what it removes", s0.includes("Unlocking removes the administrator-owned rule"));
    await (await button(page, "Unlock (administrator prompt)")).click();
    await sleep(300);
    const s = await text(page);
    ok("with no prompt available the exact command is shown", s.includes("sudo '/Library/PrivilegedHelperTools/com.gitswitch.lock-helper'") && s.includes("--sha256"));
    ok("  with an 'I ran it' button", (await button(page, "I ran it")) !== null);
    await (await button(page, "I ran it")).click();
    await sleep(300);
    const s2 = await text(page);
    ok("  which reads the result and updates the state", s2.includes("Unlocked 1 repository.") && (await segment(page, "Allowed"))?.checked === true);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Identity");
  {
    const page = await open(SCENARIOS.wrongIdentity);
    const s = await text(page);
    ok("a wrong identity blocks the commit", s.includes("wrong identity"));
    ok("it names the expected email", s.includes("me@work.test"));
    ok("and says the repo's own config is overriding the profile", s.includes(".git/config"));
    ok("the Commit button is disabled", await (await button(page, "Commit")).evaluate((b) => b.disabled));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.pushedAlready);
    const s = await text(page);
    ok("amend is disabled once the commit is pushed", s.includes("already pushed"));
    const cb = await page.$('input[aria-label="Amend the last commit"]');
    ok("and the checkbox itself is disabled", await cb.evaluate((el) => el.disabled));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("An interrupted merge");
  {
    const page = await open(SCENARIOS.rebasing);
    const s0 = await text(page);
    ok("a stopped rebase offers Continue", s0.includes("Rebase in progress") && (await button(page, "Continue")) !== null);
    ok("  enabled when nothing is conflicted", !(await (await button(page, "Continue")).evaluate((b) => b.disabled)));
    // HEAD is detached for the whole rebase; the remedy is Continue/Abort, not the branch picker.
    ok("  the header names the rebase, not a detached HEAD", s0.split("Rebase in progress").length >= 3 && !s0.includes("not on a branch"));
    ok("  nobody is told to switch branches", !s0.includes("Switch to a branch") && !s0.includes("Check out a branch first"));
    ok("  and no Choose a branch is offered", (await page.evaluate(() => [...document.querySelectorAll("button")].filter((b) => b.innerText.trim() === "Choose a branch").length)) === 0);
    ok("  Preview update waits, saying to finish or abort the rebase first", await (await button(page, "Preview update")).evaluate((b) => b.disabled && b.title.includes("Finish or abort the rebase first")));
    // The backend allows a plain commit during a rebase (it is one way to finish a stopped step), so the box asks for a message, not for a branch.
    ok("  the commit box asks for a message, not for a branch", s0.includes("A commit needs a message.") && !s0.includes("Switch to a branch before committing"));
    await (await button(page, "Continue")).click();
    await sleep(300);
    ok("  and continuing reports its outcome", (await text(page)).includes("continue ok"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.rebasingConflicted);
    const c = await button(page, "Continue");
    ok("with a conflicted file Continue is disabled", await c.evaluate((b) => b.disabled));
    ok("  and says why", (await c.evaluate((b) => b.title)).includes("Resolve and stage"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.merging);
    ok("a merge has no Continue button (the commit finishes it)", (await button(page, "Continue")) === null);
    const s = await text(page);
    ok("a banner says a merge is in progress", s.includes("Merge in progress"));
    ok("abort is offered", s.includes("Abort"));
    ok("the commit box says this finishes the merge", s.includes("finishes the merge"));
    ok("committing is allowed even with nothing staged", !(await (await button(page, "Commit")).evaluate((b) => b.disabled)));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.conflicted);
    const s = await text(page);
    ok("conflicts get their own group", s.includes("Conflicts"));
    ok("named in plain words", s.includes("both modified"));
    ok("commit is blocked while they remain", s.includes("still have conflicts") || s.includes("Resolve and stage"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Update: the latest from upstream, your work on top");
  const stashBox = (page) =>
    page.evaluate(() => document.querySelector('input[aria-label="Stash uncommitted changes during the update"]')?.click() ?? false);
  const drawer = (page) => page.$('[role="dialog"][aria-label="Update"]');
  const drawerText = (page) => page.evaluate(() => document.querySelector('[role="dialog"][aria-label="Update"]')?.innerText ?? "");
  const waitDrawer = (page) => page.waitForFunction(() => !!document.querySelector('[role="dialog"][aria-label="Update"]'), { timeout: 4000 });
  const planCalls = (page) => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_sync_plan").map((c) => c.args));
  {
    const page = await open(SCENARIOS.diverged);
    const s0 = await text(page);
    ok("the strip names the preview as the next step on a branch that is behind", s0.includes("Next: Preview update"));
    ok("Update now is not offered before a plan exists", (await button(page, "Update now")) === null);
    const pv = await button(page, "Preview update");
    ok("Preview update is offered and enabled", pv !== null && !(await pv.evaluate((b) => b.disabled)));
    await pv.click();
    await waitDrawer(page);
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    const s1 = await drawerText(page);
    ok("the drawer explains itself", s1.includes("your work on top") && s1.includes("Nothing is pushed"));
    ok("the plan is a sentence", s1.includes("Your 2 commits are replayed on top of 3 new commits from origin/main"));
    ok("  the submodule table names A's rebase and its base", s1.includes("rebase your commits onto 7a9d38a"));
    ok("  and B's fast-forward", s1.includes("fast-forward to 3c3c3c3"));
    ok("  a skipped gitlink is collapsed, not hidden", s1.includes("1 gitlink left alone"));
    ok("  the stash choice shows the count", s1.includes("Stash my 1 uncommitted change during the update"));
    ok("  file overlap is a warning, not a blocker", s1.includes("Both sides changed shared.txt") && (await button(page, "Update now")) !== null);
    ok("  Update now is enabled", !(await (await button(page, "Update now")).evaluate((b) => b.disabled)));
    ok("  what git will run is one click away", s1.includes("What git will run"));
    const localDay = await page.evaluate((iso) => {
      const d = new Date(iso);
      const p = (n) => String(n).padStart(2, "0");
      return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
    }, "2026-09-23T10:30:00+02:00");
    ok("  the picture says who did what and when (in this zone), and which commits are only pointer bumps", s1.includes("3 new commits below") && s1.includes("Other") && s1.includes(localDay) && s1.includes("pointer bump"), localDay);
    ok("  and what the submodules bring in and own", s1.includes("coming in: 2") && s1.includes("yours: 1"));
    ok("  the rails draw both sides from the base and say what comes after", s1.includes("You · main") && s1.includes("where you and the remote last agreed · e81d1c5") && s1.includes("After Update now") && s1.includes("Your 2 commits are put back on top of the 3 new ones"));
    ok("  nothing is shown as dropped when nothing is", !s1.includes("already there — dropped") && (await page.evaluate(() => document.querySelectorAll('[aria-label="Update preview"] .line-through').length)) === 0);
    const c0 = await planCalls(page);
    ok("  opening the drawer assessed once, fetching, with stashing on", c0.length === 1 && c0[0].fetch === true && c0[0].stash === true);
    await stashBox(page);
    await page.waitForFunction(() => document.body.innerText.includes("Turn on stashing"), { timeout: 4000 });
    const c1 = await planCalls(page);
    ok("turning stashing off re-plans without a second fetch", c1.length === 2 && c1[1].fetch === false && c1[1].stash === false);
    ok("  and the dirty tree becomes a named blocker with a remedy button", (await drawerText(page)).includes("Turn on stashing") && (await button(page, "Turn on stashing")) !== null);
    ok("  that disables Update now", await (await button(page, "Update now")).evaluate((b) => b.disabled));
    await (await button(page, "Turn on stashing")).click();
    await page.waitForFunction(() => !document.body.innerText.includes("Turn on stashing"), { timeout: 4000 });
    ok("  the remedy turns it back on and re-plans", (await planCalls(page)).length === 3);
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Updated:"), { timeout: 4000 });
    ok("Update now closes the drawer", (await drawer(page)) === null);
    const s2 = await text(page);
    const run = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_sync_run"));
    ok("  and sends the plan's fingerprint and the choices", run && run.args.stash === true && run.args.bundles === false && run.args.fingerprint.length === 3);
    ok("  with the page's large-file choice, as Pull takes it", run?.args.withLfs === true, JSON.stringify(run?.args));
    ok("the result is a table per repository, in the outcome's own words", s2.includes("this repository") && s2.includes("aaaa111 → cccc333") && s2.includes("rebased onto 7a9d38a") && s2.includes("fast-forwarded"));
    ok("  every check passed, said so", s2.includes("All 6 checks passed"));
    ok("  the backups are one click away, with the undo commands", s2.includes("Backups: 2 branches"));
    await page.evaluate(() => [...document.querySelectorAll("summary")].find((x) => x.innerText.includes("Backups"))?.click());
    await sleep(200);
    ok("  the undo command is exact", (await text(page)).includes("git -C /repos/gitswitch reset --hard gitswitch-before-update-"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { syncPlan: SYNC_PLAN_BLOCKED, syncPlanAfterRemedy: SYNC_PLAN });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("checked out at"), { timeout: 4000 });
    const s = await drawerText(page);
    ok("a blocker is a sentence with the remedy", s.includes("commit that pointer (Record submodule pointers)"));
    const rec = await button(page, "Record submodule pointers");
    ok("  and a button keyed on the remedy code, not the sentence", rec !== null);
    const b = await button(page, "Update now");
    ok("  Update now stays disabled, saying why", (await b.evaluate((x) => x.disabled)) && (await b.evaluate((x) => x.title)).includes("blockers"));
    await rec.click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_record_pointers"), { timeout: 4000 });
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "changes_sync_plan").length >= 2, { timeout: 4000 });
    const last = (await planCalls(page)).at(-1);
    ok("  the remedy records the pointers, then plans again without a second fetch", last.fetch === false);
    await page.waitForFunction(() => (document.querySelector('[role="dialog"][aria-label="Update"]')?.innerText ?? "").includes("replayed on top"), { timeout: 4000 });
    await sleep(300);
    const after = await drawerText(page);
    ok("  the new plan replaces the blocked one and stays", after.includes("Your 2 commits are replayed on top of 3 new commits") && !after.includes("checked out at"), after.slice(0, 200));
    ok("  Update now is enabled on it", !(await (await button(page, "Update now")).evaluate((b) => b.disabled)));
    ok("  made for the moved HEAD, so the fresh status keeps it and nothing fetched again", (await page.evaluate(() => window.__SCENARIO__.head_oid)) === "bbbb2222" && (await planCalls(page)).length === 2, `${(await planCalls(page)).length} plan calls`);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { syncPlan: SYNC_PLAN_NOTHING });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Nothing to update"), { timeout: 4000 });
    ok("nothing to do is said in words, with no Update now", (await button(page, "Update now")) === null);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.unpublished);
    const b = await button(page, "Preview update");
    ok("without an upstream Preview update is disabled", await b.evaluate((x) => x.disabled));
    ok("  and says what to do", (await b.evaluate((x) => x.title)).includes("no upstream to update from"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { hang: true });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Updating"), { timeout: 4000 });
    const s = await text(page);
    ok("a running update is a job banner that survives leaving the page", s.includes("Updating — fetching, rebasing") && s.includes("keeps running"));
    await page.close();
  }
  {
    // An Update that downloads a submodule upstream added shows its progress
    // on the strip, as Download does — and the line goes when that download
    // is over, while the Update carries on.
    const page = await open(SCENARIOS.diverged, {
      hang: true,
      cloneProgress: { phase: "Submodules", percent: 37, done: 0, total: 1, bytes: "10.00 MiB", rate: "2.00 MiB/s", detail: "libs/new", estimate_kb: 1258291, eta_secs: 603 },
    });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Submodules 37%"), { timeout: 4000 }).catch(() => null);
    const s = await text(page);
    ok("while an Update downloads a submodule, the strip says which one, how much has arrived and how long is left",
      s.includes("Updating") && s.includes("Submodules 37%") && s.includes("libs/new") && s.includes("10.00 MiB of about 1.2 GB") && s.includes("about 10 min left"),
      s.slice(s.indexOf("Updating"), s.indexOf("Updating") + 200));
    await page.evaluate(() => { window.__NO_PROGRESS__ = true; });
    await page.waitForFunction(() => !document.body.innerText.includes("Submodules 37%"), { timeout: 4000 }).catch(() => null);
    const s2 = await text(page);
    ok("  and the line goes when that download is over, while the Update runs on", !s2.includes("Submodules 37%") && s2.includes("Updating"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { syncOutcome: SYNC_OUTCOME_UNRECORDED });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Updated:"), { timeout: 4000 });
    ok("a submodule ahead of the recorded pointer is named", (await text(page)).includes("B is checked out ahead of what this branch records"));
    const rec = await button(page, "Record submodule pointers");
    ok("  with a button to record it", rec !== null);
    await rec.click();
    await sleep(300);
    ok("  which makes the commit", (await text(page)).includes("Recorded 1 submodule pointer(s)."));
    await page.close();
  }
  {
    const refused = { ok: false, headline: "Upstream moved since you assessed. Assess again, then update.", detail: "", refusal: { code: "assess-again", message: "Upstream moved since you assessed. Assess again, then update." } };
    const page = await open(SCENARIOS.diverged, { syncRunResult: refused });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("moved while you were reading"), { timeout: 4000 });
    ok("a run refused because upstream moved re-opens the drawer and says so", (await drawer(page)) !== null);
    const calls = await planCalls(page);
    ok("  with a second plan that fetched again", calls.length === 2 && calls[1].fetch === true);
    const outside = await page.evaluate(() => {
      const b = document.body.cloneNode(true);
      b.querySelector('[role="dialog"][aria-label="Update"]')?.remove();
      return b.textContent || "";
    });
    ok("  and no error card or toast for it outside the drawer", !outside.includes("Upstream moved since you assessed") && !outside.includes("That didn't work"));
    await page.close();
  }
  {
    // The plan re-made because upstream moved has a blocker, and its remedy button takes the drawer down.
    const refused = { ok: false, headline: "Upstream moved since you assessed. Assess again, then update.", detail: "", refusal: { code: "assess-again", message: "Upstream moved since you assessed. Assess again, then update." } };
    const blocked = {
      ...SYNC_PLAN,
      can_run: false,
      blockers: ["Untracked file would be overwritten by incoming commits: conf. Move or remove them first."],
      superproject: { ...SYNC_PLAN.superproject, remedies: ["untracked-in-the-way"] },
    };
    const page = await open(SCENARIOS.diverged, { syncRunResult: refused, syncPlanSecond: blocked });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("moved while you were reading"), { timeout: 4000 });
    await page.waitForFunction(() => [...document.querySelectorAll('[role="dialog"][aria-label="Update"] button')].some((b) => b.innerText.trim() === "Go to Changes"), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll('[role="dialog"][aria-label="Update"] button')].find((b) => b.innerText.trim() === "Go to Changes").click());
    await page.waitForFunction(() => !document.querySelector('[role="dialog"][aria-label="Update"]'), { timeout: 4000 });
    ok("a remedy that leads away takes the drawer down", (await drawer(page)) === null);
    await (await button(page, "Preview update")).click();
    await waitDrawer(page);
    await sleep(500);
    const again = await drawerText(page);
    ok("  opened again, the plan that was kept is shown", again.includes("Untracked file would be overwritten"), again.slice(0, 200));
    ok("  without saying upstream moved while you were reading — this time nothing did", !again.includes("moved while you were reading"), again.slice(0, 300));
    ok("  and without fetching again", (await planCalls(page)).length === 2, `${(await planCalls(page)).length} plan calls`);
    await page.close();
  }
  {
    // git spells the upstream "remotes/origin/main" when a local branch or tag has the short name too.
    const page = await open({ ...SCENARIOS.diverged, upstream: "remotes/origin/main" });
    await (await button(page, "Preview update")).click();
    // (No throw on a timeout: a plan that never stays is what the checks below report.)
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 }).catch(() => {});
    await sleep(800);
    const s = await drawerText(page);
    ok("a plan stays when git spells the upstream with its remotes/ prefix", s.includes("replayed on top"), s.slice(0, 200));
    const b = await button(page, "Update now");
    ok("  and can be run", b !== null && !(await b.evaluate((x) => x.disabled)));
    ok("  after one assessment, not one after another", (await planCalls(page)).length === 1, `${(await planCalls(page)).length} plan calls`);
    await page.evaluate(() => [...document.querySelectorAll("summary")].find((x) => x.innerText.includes("What git will run"))?.click());
    await sleep(150);
    const cmds = await drawerText(page);
    ok("  the fetch names the remote, not the word remotes", cmds.includes("git fetch origin") && !cmds.includes("git fetch remotes"), cmds.slice(cmds.indexOf("git fetch"), cmds.indexOf("git fetch") + 40));
    ok("  the rebase line keeps git's own unambiguous spelling", cmds.includes("git rebase --autostash remotes/origin/main"));
    ok("  and the title uses the plain name", cmds.includes("the latest from origin/main, your work on top") && !cmds.includes("from remotes/origin/main"));
    await page.close();
  }
  {
    // Three ways a commit of mine can be absent afterwards, and the card says which is which.
    const c = SYNC_PLAN.superproject.own_commits[0];
    const outcome = {
      ...SYNC_OUTCOME,
      own_kept_total: 2,
      own_dropped: [
        { ...c, oid: "d1".repeat(20), subject: "same as upstream", already_upstream: true },
        { ...c, oid: "d2".repeat(20), subject: "edit readme", already_upstream: false, resolved_away: true },
        { ...c, oid: "d3".repeat(20), subject: "wip", already_upstream: false },
      ],
      verified: { ...SYNC_OUTCOME.verified, own_on_top: false },
    };
    const result = { ok: false, headline: "Update finished, but a check did not pass — read the details.", detail: "", sync: outcome };
    const page = await open(SCENARIOS.diverged, { syncRunResult: result });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("a check did not pass"), { timeout: 4000 });
    const s = (await text(page)).replace(/\s+/g, " ");
    ok("a commit upstream already had is said to be left out for that reason", s.includes("Left out because upstream already had it: same as upstream."), s.slice(0, 300));
    ok("  one resolved into nothing is said to be that", s.includes("Left out because nothing of it remained once its conflict was resolved: edit readme."));
    ok("  and one that is simply gone is called missing, with where the original is", s.includes("Missing after the replay and not found upstream: wip. The original is on the backup branch — see Backups."));
    ok("  the row counts them apart", s.includes("2 of yours kept, 2 left out, 1 missing"));
    ok("  nothing missing is filed under upstream-had-it", !/already had (it|them): [^.]*wip/.test(s));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.behindOnly, { pullResult: { ok: true, headline: "Pulled 2 commit(s) (fast-forward).", detail: "", pull: PULL_MOVED_OUTCOME } });
    await (await button(page, "Pull")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Pulled 2 commit(s)"), { timeout: 4000 });
    const s = await text(page);
    ok("a pull that moved submodule pointers says so", s.includes("2 submodule pointers moved"));
    const up = await button(page, "Update…");
    ok("  and offers the update", up !== null);
    await up.click();
    await waitDrawer(page);
    ok("  which opens the drawer", (await drawer(page)) !== null);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPaused);
    const s = await text(page);
    ok("a paused update replaces the plain rebase banner", s.includes("Update paused") && !s.includes("Rebase in progress"));
    ok("  with the hint", s.includes("Resolve the conflicts listed on this page"));
    ok("  and the file", s.includes("shared.txt"));
    ok("  Preview update waits for the rebase to finish", await (await button(page, "Preview update")).evaluate((b) => b.disabled));
    const c = await button(page, "Continue update");
    ok("  Continue update is disabled while conflicts remain", await c.evaluate((b) => b.disabled));
    ok("  saying why, with the count", (await c.evaluate((b) => b.title)).includes("Resolve and stage") && (await c.evaluate((b) => b.title)).includes("1 left"));
    ok("  the conflict list is there to resolve them", s.includes("Conflicts") && s.includes("both modified"));
    await page.evaluate(() => {
      window.__SCROLLED__ = [];
      const orig = Element.prototype.scrollIntoView;
      Element.prototype.scrollIntoView = function (...a) {
        window.__SCROLLED__.push(this.id || this.tagName);
        return orig.apply(this, a);
      };
    });
    await (await button(page, "Go to the conflicts")).click();
    await sleep(400);
    ok(
      "  Go to the conflicts scrolls to the list and lands on it",
      await page.evaluate(() => {
        const el = document.getElementById("conflicts");
        if (!el || !window.__SCROLLED__.includes("conflicts")) return false;
        const r = el.getBoundingClientRect();
        return r.top >= 0 && r.top < window.innerHeight;
      }),
      JSON.stringify(await page.evaluate(() => window.__SCROLLED__))
    );
    const rowButtons = await page.evaluate(() => {
      const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes("shared.txt"));
      return row ? [...row.querySelectorAll("button")].map((b) => b.innerText.trim()) : null;
    });
    ok(
      "  the conflicted row offers the editor and the file manager beside Keep mine / Take theirs",
      rowButtons?.includes("Open in editor") && rowButtons?.includes("Reveal") && rowButtons?.includes("Keep mine") && rowButtons?.includes("Take theirs"),
      JSON.stringify(rowButtons)
    );
    await clickInRow(page, "shared.txt", { text: "Open in editor" });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_open_path"), { timeout: 4000 });
    const opened = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_open_path").args);
    ok("  Open in editor asks the backend to open that path inside the repository", opened.repoPath === "/repos/gitswitch" && opened.path === "shared.txt" && opened.reveal === false, JSON.stringify(opened));
    await page.evaluate(() => {
      const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes("shared.txt"));
      row.querySelector("button").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Conflict view"]'), { timeout: 4000 });
    const cv = await page.evaluate(() => document.querySelector('[aria-label="Conflict view"]').innerText);
    ok("  clicking the row shows the conflict by its sides, named as on the list", cv.includes("mine = your commit being replayed") && cv.includes("theirs = origin/main") && cv.includes("shared line 2 (mine)") && cv.includes("shared line 2 (upstream)"));
    ok("  with the count of conflicts left in the file", cv.includes("1 conflict left in the file"));
    await page.click('button[aria-label="Close"]');
    await sleep(100);
    await (await button(page, "Abort update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Update aborted."), { timeout: 4000 });
    ok("  Abort update reports itself", true);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPausedInSub);
    const s = await text(page);
    ok("paused inside a submodule says so", s.includes("Update paused inside A"));
    const c = await button(page, "Continue update");
    ok("  Continue update waits for A's own conflict", await c.evaluate((b) => b.disabled));
    const o = await button(page, "Open A");
    ok("  and Open A hands over", o !== null);
    await o.click();
    await sleep(400);
    ok(
      "  making A the scope on the Changes tab",
      (await page.evaluate(() => JSON.parse(localStorage.getItem("gitswitch:repo.sub") ?? '""'))) === "A" && (await page.evaluate(() => location.pathname)) === "/repo/changes"
    );
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncHandedOver);
    const s = await text(page);
    ok("a pointer the update handed over says what is needed, not that something is inside it", s.includes("Update paused: the pointer of A needs staging.") && !s.includes("Update paused inside A"));
    ok("  with the backend's sentence about where to put it", s.includes("Put it where you want it"));
    const c = await button(page, "Continue update");
    ok("  Continue update waits until it is staged, saying where", (await c.evaluate((b) => b.disabled)) && (await c.evaluate((b) => b.title)).includes("Stage A on this tab first"));
    ok("  the list it is in is one click away, and so is A itself", (await button(page, "Go to the conflicts")) !== null && (await button(page, "Open A")) !== null);
    const rowTitles = await page.evaluate(() => {
      const row = [...document.querySelectorAll("li")].find((r) => r.innerText.trim().startsWith("A") || r.innerText.includes("A\n"));
      return row ? [...row.querySelectorAll("button")].map((b) => b.title) : null;
    });
    ok("  and its row can be marked resolved", rowTitles !== null && rowTitles.includes("Mark resolved"), JSON.stringify(rowTitles));
    // Opening the row shows the pointer's diff: a pointer has no conflict sides to split.
    await page.evaluate(() => {
      const row = [...document.querySelectorAll("#conflicts li")].find((r) => r.innerText.trim().startsWith("A") || r.innerText.includes("A\n"));
      row.querySelector("button").click();
    });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_file_diff"), { timeout: 4000 }).catch(() => {});
    const opened = await page.evaluate(() => ({
      diff: window.__CALLS__.filter((c) => c.cmd === "changes_file_diff").map((c) => c.args.path),
      view: window.__CALLS__.filter((c) => c.cmd === "changes_conflict_view").length,
    }));
    ok("  opening it reads the pointer's diff, not a conflict view", opened.diff.includes("A") && opened.view === 0, JSON.stringify(opened));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPausedChild);
    const s = await text(page);
    ok("the submodule's own page knows the parent's update is paused here", s.includes("An update of gitswitch is paused inside this submodule"));
    ok("  and offers to go back to the parent", (await button(page, "Open gitswitch")) !== null);
    ok("  without a Continue update of its own", (await button(page, "Continue update")) === null);
    ok("  and names theirs as the parent's upstream's record for A", s.includes("origin/main's A (7a9d38a)"));
    await page.close();
  }
  {
    const refusal = { code: "conflict-markers", message: "shared.txt still contains conflict markers (lines 2, 6). Finish the edit, or Keep mine / Take theirs." };
    const page = await open(SCENARIOS.syncPaused, { stageRefusal: refusal });
    ok("Mark resolved on a file with its markers still in it", await clickInRow(page, "shared.txt", { title: "Mark resolved" }));
    await page.waitForFunction(() => document.body.innerText.includes("still contains conflict markers"), { timeout: 4000 });
    ok("  is refused by name, with the lines", (await text(page)).includes("shared.txt still contains conflict markers (lines 2, 6)"));
    const anyway = await button(page, "Mark resolved anyway");
    ok("  and offers to do it anyway", anyway !== null);
    await anyway.click();
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "changes_stage").length >= 2, { timeout: 4000 });
    const calls = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stage").map((c) => c.args));
    ok("  which stages the same file with markers allowed", calls.length === 2 && calls[0].allowMarkers === false && calls[1].allowMarkers === true && calls[1].paths[0] === "shared.txt", JSON.stringify(calls));
    await page.close();
  }

  {
    const page = await open(SCENARIOS.diverged, { syncPlan: SYNC_PLAN_DROPPED });
    await (await button(page, "Preview update")).click();
    await waitDrawer(page);
    await page.waitForFunction(() => document.body.innerText.includes("After Update now"), { timeout: 4000 });
    const s = await drawerText(page);
    const struck = await page.evaluate(() => [...document.querySelectorAll('[aria-label="Update preview"] .line-through')].map((e) => e.innerText.trim()));
    ok("a commit upstream already has is struck through and named as dropped", s.includes("already there — dropped") && JSON.stringify(struck) === '["seal work"]', JSON.stringify(struck));
    ok("  and what stays on top is counted without it", s.includes("your 1 commit on top") && s.includes("1 commit of yours is dropped because origin/main already has it") && s.includes("Your 1 commit is put back on top of the 3 new ones"), s.slice(0, 400));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { syncPlan: SYNC_PLAN_MANY });
    await (await button(page, "Preview update")).click();
    await waitDrawer(page);
    await page.waitForFunction(() => document.body.innerText.includes("After Update now"), { timeout: 4000 });
    const s = await drawerText(page);
    ok("with more commits than are listed, the count is of all of them, less the ones upstream has", s.includes("your 32 commits on top") && s.includes("3 commits of yours are dropped"), s.slice(0, 500));
    ok("  and the rail says how many more there are and how many of those are already there", s.includes("+33 more (3 of them already in origin/main)"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { syncPlan: SYNC_PLAN_LEVEL });
    await (await button(page, "Preview update")).click();
    await waitDrawer(page);
    await page.waitForFunction(() => document.body.innerText.includes("After Update now"), { timeout: 4000 });
    const s = await drawerText(page);
    ok("level with upstream, the preview says the branch is not touched", s.includes("already level with origin/main: it is not touched and no commit id changes"));
    ok("  and promises no new ids", !s.includes("new ids") && !s.includes("Their ids change"));
    const willRun = await page.evaluate(() => [...document.querySelectorAll('[role="dialog"] details')].find((d) => d.querySelector("summary")?.innerText.includes("What git will run"))?.textContent ?? "");
    ok("  nor a rebase among the commands", willRun.includes("git fetch") && !willRun.includes("git rebase origin/main"), willRun);
    await page.close();
  }
  {
    // The plan stays in the store; the repository itself may stop allowing it.
    const page = await open(SCENARIOS.diverged);
    await (await button(page, "Preview update")).click();
    await waitDrawer(page);
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    ok("with a plan and nothing in the way, Update now is live", !(await (await button(page, "Update now")).evaluate((b) => b.disabled)));
    await page.evaluate((op) => {
      window.__SCENARIO__ = { ...window.__SCENARIO__, operation: op };
    }, MERGE_OP);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Refresh").click());
    await page.waitForFunction(() => (document.querySelector('[role="dialog"][aria-label="Update"]')?.innerText ?? "").includes("Finish or abort the merge first"), { timeout: 4000 });
    ok("a merge started meanwhile blocks the kept plan: Update now is off", await (await button(page, "Update now")).evaluate((b) => b.disabled));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPausedAligning);
    const b = await button(page, "Preview update");
    ok("an update paused with no git operation in progress still owns the repository", b !== null && (await b.evaluate((x) => x.disabled)) && (await b.evaluate((x) => x.title)).includes("An update is paused here"), b ? await b.evaluate((x) => x.title) : "no button");
    await page.close();
  }
  {
    const refusal = { code: "conflict-markers", message: "shared.txt still contains conflict markers (line 2). Finish the edit, or Keep mine / Take theirs." };
    const page = await open(SCENARIOS.syncPausedResolved, { syncContinueRefusal: refusal });
    await (await button(page, "Continue update")).click();
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Continue update anyway"), { timeout: 4000 });
    ok("Continue update refused for markers offers its own retry", (await text(page)).includes("shared.txt still contains conflict markers (line 2)"));
    await (await button(page, "Continue update anyway")).click();
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "changes_sync_continue").length >= 2, { timeout: 4000 });
    const calls = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_sync_continue").map((c) => c.args));
    ok("  which continues the update with markers allowed", calls.length === 2 && calls[0].allowMarkers === false && calls[1].allowMarkers === true, JSON.stringify(calls));
    await page.close();
  }
  {
    const refusal = { code: "conflict-markers", message: "a.txt still contains conflict markers (line 1). Finish the edit, or Keep mine / Take theirs." };
    const page = await open(SCENARIOS.rebasing, { continueRefusal: refusal });
    await (await button(page, "Continue")).click();
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Continue anyway"), { timeout: 4000 });
    await (await button(page, "Continue anyway")).click();
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "changes_continue").length >= 2, { timeout: 4000 });
    const calls = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_continue").map((c) => c.args));
    ok("a plain Continue refused for markers offers Continue anyway, the same call with markers allowed", calls.length === 2 && calls[0].allowMarkers === false && calls[1].allowMarkers === true && calls[1].repoPath === "/repos/gitswitch", JSON.stringify(calls));
    await page.close();
  }
  {
    // Update now, then away to another scope before the refusal comes back.
    const refused = { ok: false, headline: "Upstream moved since you assessed. Assess again, then update.", detail: "", refusal: { code: "assess-again", message: "Upstream moved since you assessed. Assess again, then update." } };
    const page = await open(SCENARIOS.diverged, { syncRunHold: true, submodules: SUBMODULES, subInner: SUB_INNER });
    await (await button(page, "Preview update")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Update now")).click();
    await page.waitForFunction(() => typeof window.__RELEASE_RUN__ === "function", { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll('a[role="tab"]')].find((a) => a.innerText.trim().startsWith("Submodules")).click());
    await page.waitForFunction(() => [...document.querySelectorAll("tr button")].some((b) => b.innerText.trim().startsWith("Work inside")), { timeout: 4000 });
    await page.evaluate(() => {
      const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes("delivery"));
      [...row.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Work inside")).click();
    });
    await page.waitForFunction(() => JSON.parse(localStorage.getItem("gitswitch:repo.sub") ?? '""') === "delivery", { timeout: 4000 });
    await sleep(300);
    const before = (await planCalls(page)).length;
    await page.evaluate((r) => window.__RELEASE_RUN__(r), refused);
    await sleep(500);
    ok("a run refused while another scope is on screen does not open the drawer there", (await drawer(page)) === null);
    ok("  nor assess the repository that is on screen", (await planCalls(page)).length === before, `${(await planCalls(page)).length} vs ${before}`);
    // Back to the repository the run belonged to: the scope picker, "This repository".
    await page.evaluate(() => {
      const label = [...document.querySelectorAll("label")].find((l) => l.innerText.trim() === "Scope");
      label.parentElement.querySelector("button").click();
    });
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "This repository"), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "This repository").click());
    const told = await page
      .waitForFunction(() => (document.querySelector('[role="dialog"][aria-label="Update"]')?.innerText ?? "").includes("moved while you were reading"), { timeout: 5000 })
      .then(() => true, () => false);
    ok(
      "back on it, the refusal is told: the drawer opens and says upstream moved",
      told && (await drawer(page)) !== null,
      JSON.stringify(await page.evaluate(() => ({ sub: localStorage.getItem("gitswitch:repo.sub"), path: location.pathname, dialog: (document.querySelector('[role="dialog"]')?.innerText ?? "none").slice(0, 200), body: document.body.innerText.slice(0, 300) })))
    );
    const after = await planCalls(page);
    ok("  with one new plan, fetched again, for that repository", after.length === before + 1 && after.at(-1).fetch === true && after.at(-1).repoPath === "/repos/gitswitch", JSON.stringify(after.at(-1)));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { width: 700 });
    await (await button(page, "Preview update")).click();
    await waitDrawer(page);
    await page.waitForFunction(() => document.body.innerText.includes("After Update now"), { timeout: 4000 });
    const over = await page.evaluate(() => {
      const d = document.querySelector('[role="dialog"][aria-label="Update"]');
      const inner = [...d.querySelectorAll("*")].filter((el) => el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).overflowX !== "auto" && getComputedStyle(el).overflowX !== "scroll" && !el.closest(".overflow-x-auto"));
      return { page: document.documentElement.scrollWidth > window.innerWidth + 1, drawer: d.scrollWidth > d.clientWidth + 1, inner: inner.length };
    });
    ok("the drawer with the rails fits a 700px window", !over.page && !over.drawer, JSON.stringify(over));
    ok("  and nothing inside it is clipped sideways", over.inner === 0, `${over.inner} element(s) wider than their box`);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Diff viewer");
  {
    const page = await open(SCENARIOS.dirty);
    ok("clicking a file opens its diff", await clickInRow(page, "src/mod.ts", { index: 0 }));
    await page.waitForFunction(() => document.body.innerText.includes("@@ -1,3 +1,4 @@"), { timeout: 4000 });
    const s = await text(page);
    ok("hunk headers and both line kinds render", s.includes("+added line") && s.includes("-removed line"));
    ok("with +/- counts in the header", s.includes("+1") && s.includes("−1"));
    await page.keyboard.press("Escape");
    await sleep(200);
    ok("Escape closes it", !(await text(page)).includes("@@ -1,3 +1,4 @@"));
    await clickInRow(page, "logo.png", { index: 0 });
    await page.waitForFunction(() => document.body.innerText.includes("Binary file"), { timeout: 4000 });
    ok("a binary file says so instead of rendering", true);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Diffs: unified as before, or side by side — one choice, remembered, for every diff");
  {
    const SQL = {
      ...DIFF_TEXT,
      path: "db/q.sql",
      lines: [
        { kind: "meta", text: "diff --git a/db/q.sql b/db/q.sql" },
        { kind: "meta", text: "--- a/db/q.sql" },
        { kind: "meta", text: "+++ b/db/q.sql" },
        { kind: "hunk", text: "@@ -10,4 +10,5 @@ select" },
        { kind: "context", text: " select 1;" },
        // A removed SQL comment: git writes "-" + "-- old"; read by the hunk's counts it is a removed line.
        { kind: "meta", text: "--- old comment" },
        { kind: "del", text: "-where a = 1" },
        { kind: "add", text: "+-- new comment" },
        { kind: "add", text: "+where a = 2" },
        { kind: "add", text: "+  and b = 3" },
        { kind: "context", text: " limit 5;" },
        { kind: "meta", text: "\\ No newline at end of file" },
      ],
    };
    const view = (page) =>
      page.evaluate(() => ({
        pre: !!document.querySelector("pre"),
        table: !!document.querySelector('[data-diff-view="split"]'),
        toggles: document.querySelectorAll("[data-diff-toggle]").length,
        stored: localStorage.getItem("gitswitch:diff.view"),
      }));
    const rowsOf = (page) =>
      page.evaluate(() =>
        [...document.querySelectorAll('[data-diff-view="split"] tr')].map((r) =>
          r.dataset.splitRow === "full"
            ? r.innerText.trim()
            : [r.dataset.old, r.querySelector('[data-side="old"]').innerText, r.dataset.new, r.querySelector('[data-side="new"]').innerText]
        )
      );
    const pick = (page, label) => page.evaluate((label) => [...document.querySelectorAll("[data-diff-toggle] button")].find((b) => b.innerText.trim() === label)?.click(), label);

    const page = await open(SCENARIOS.dirty, { diffText: SQL });
    await clickInRow(page, "src/mod.ts", { index: 0 });
    await page.waitForFunction(() => document.body.innerText.includes("@@ -10,4 +10,5 @@"), { timeout: 4000 });
    let v = await view(page);
    ok("a diff opens unified, as before", v.pre && !v.table && v.toggles === 1, JSON.stringify(v));
    await pick(page, "Side by side");
    await sleep(150);
    v = await view(page);
    ok("Side by side shows it as a table instead", v.table && !v.pre, JSON.stringify(v));
    ok("  and remembers the choice", v.stored === '"split"');
    const rows = await rowsOf(page);
    ok("  headers and the hunk line span the table", rows[0] === "diff --git a/db/q.sql b/db/q.sql" && rows[3] === "@@ -10,4 +10,5 @@ select", JSON.stringify(rows.slice(0, 4)));
    ok("  a context line sits on both sides with both numbers", JSON.stringify(rows[4]) === JSON.stringify(["10", "select 1;", "10", "select 1;"]), JSON.stringify(rows[4]));
    ok("  removed lines pair with added ones, row by row — a removed \"--\" line too", JSON.stringify(rows[5]) === JSON.stringify(["11", "-- old comment", "11", "-- new comment"]) && JSON.stringify(rows[6]) === JSON.stringify(["12", "where a = 1", "12", "where a = 2"]), JSON.stringify(rows.slice(5, 8)));
    ok("  an added line with nothing opposite leaves its left side empty", JSON.stringify(rows[7]) === JSON.stringify(["", "", "13", "  and b = 3"]), JSON.stringify(rows[7]));
    ok("  the numbering carries on after the change", JSON.stringify(rows[8]) === JSON.stringify(["13", "limit 5;", "14", "limit 5;"]), JSON.stringify(rows[8]));
    ok("  and git's \"no newline\" note spans the table", rows[9] === "\\ No newline at end of file", JSON.stringify(rows[9]));
    ok("  the removed side is red and the added side green", await page.evaluate(() => { const r = document.querySelectorAll('[data-diff-view="split"] tr[data-split-row="pair"]')[1]; return r.querySelector('[data-side="old"]').className.includes("red") && r.querySelector('[data-side="new"]').className.includes("emerald"); }));
    // UI_SHOTS=<folder>: a picture of it, for people to look at.
    if (process.env.UI_SHOTS) await page.screenshot({ path: `${process.env.UI_SHOTS}/diff-side-by-side.png` });
    await page.keyboard.press("Escape");
    await sleep(200);
    await clickInRow(page, "src/mod.ts", { index: 0 });
    await page.waitForFunction(() => !!document.querySelector('[data-diff-view="split"]'), { timeout: 4000 }).catch(() => null);
    ok("the next diff opens side by side too", (await view(page)).table);
    await pick(page, "Unified");
    await sleep(150);
    v = await view(page);
    ok("Unified brings back the view as it was", v.pre && !v.table && v.stored === '"unified"' && (await text(page)).includes("-where a = 1"));
    await page.close();

    const again = await open(SCENARIOS.dirty, { storage: { "diff.view": "split" } });
    await clickInRow(again, "src/mod.ts", { index: 0 });
    await again.waitForFunction(() => !!document.querySelector('[data-diff-view="split"]'), { timeout: 4000 }).catch(() => null);
    ok("the choice holds the next time the app opens", (await view(again)).table);
    const merge = await open(SCENARIOS.dirty, { storage: { "diff.view": "split" }, diffText: { ...DIFF_TEXT, lines: [{ kind: "hunk", text: "@@@ -1,2 -1,2 +1,3 @@@" }, { kind: "context", text: "  both" }] } });
    await clickInRow(merge, "src/mod.ts", { index: 0 });
    await merge.waitForFunction(() => document.body.innerText.includes("@@@ -1,2"), { timeout: 4000 }).catch(() => null);
    ok("a combined merge diff it can't lay out is shown unified, saying so", (await view(merge)).pre && (await text(merge)).includes("This diff can only be shown unified."));
    await again.close();
    await merge.close();

    const narrow = await open(SCENARIOS.dirty, { width: 700, storage: { "diff.view": "split" }, diffText: SQL });
    await clickInRow(narrow, "src/mod.ts", { index: 0 });
    await narrow.waitForFunction(() => !!document.querySelector('[data-diff-view="split"]'), { timeout: 4000 }).catch(() => null);
    const over = await narrow.evaluate(() => { const t = document.querySelector('[data-diff-view="split"]'); return { page: document.documentElement.scrollWidth > window.innerWidth + 1, table: t ? t.scrollWidth > t.parentElement.clientWidth + 1 : true }; });
    ok("side by side fits a 700px window: long lines wrap inside their half", !over.page && !over.table, JSON.stringify(over));
    await narrow.close();
  }

  // -----------------------------------------------------------------
  section("Discard asks first");
  {
    const page = await open(SCENARIOS.dirty);
    await (await button(page, "Discard all")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Discard these changes?"), { timeout: 4000 });
    const s = await text(page);
    ok("a confirmation appears", s.includes("Discard these changes?"));
    ok("it says the change is permanent", s.includes("cannot be undone"));
    ok("it lists the files by name", s.includes("src/mod.ts"));
    const calls = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_discard").length);
    ok("and nothing was discarded before confirming", calls === 0);
    await (await button(page, "Keep them")).click();
    await sleep(200);
    ok("cancelling really cancels", (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_discard").length)) === 0);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty);
    await clickInRow(page, "new file.txt", { title: "Discard — this cannot be undone" });
    await page.waitForFunction(() => document.body.innerText.includes("Discard these changes?"), { timeout: 4000 });
    ok("discarding a new file warns it will be deleted from disk", (await text(page)).includes("deleted from disk"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Staging");
  {
    const page = await open(SCENARIOS.dirty);
    await (await button(page, "Stage all")).click();
    await sleep(300);
    const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_stage"));
    ok("Stage all sends explicit paths, never a blank list", Array.isArray(call?.args?.paths) && call.args.paths.length > 0);
    ok("and only the files in that group", !call.args.paths.includes("new file.txt"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("A long operation survives leaving the page");
  {
    const page = await open(SCENARIOS.behindOnly, { hang: true });
    await (await button(page, "Pull")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Pulling"), { timeout: 4000 });
    ok("a running banner appears", (await text(page)).includes("Pulling"));
    ok("and says it keeps running", (await text(page)).includes("keeps running"));
    await page.evaluate(() => [...document.querySelectorAll("a")].find((a) => a.innerText.trim() === "Doctor")?.click());
    await sleep(400);
    ok("leaving the page works", !(await text(page)).includes("Not staged"));
    await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim() === "Repository")?.click());
    await page.waitForFunction(() => document.body.innerText.includes("Pulling"), { timeout: 4000 });
    ok("coming back still shows the pull running", (await text(page)).includes("Pulling"));
    ok("the elapsed time is shown", /\d+s/.test(await text(page)));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Other states");
  {
    const page = await open(SCENARIOS.truncated);
    ok("a huge untracked list says it was capped", (await text(page)).includes("only the first 2000"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.submodules, { submodules: [] });
    await sleep(300);
    ok("no submodule card when the repo has no gitlinks", !(await text(page)).includes("Submodules"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Submodules");
  {
    const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    const s = await text(page);
    ok("every gitlink is counted, not just the mapped ones", s.includes("30"));
    ok("a moved submodule states direction and size", s.includes("moved 10 commits ahead"));
    ok("  and where it sits on its own branch", s.includes("11 behind origin/main"));
    ok("a capped untracked count is shown as a floor", s.includes("2000+ untracked files"));
    ok("a clean submodule says it is up to date", s.includes("up to date with what this repo records"));
    ok("recorded and checked-out commits are both shown", s.includes("dceaf0b") && s.includes("d9b93c5"));
    ok("gitlinks with no address in .gitmodules are not called missing — no clone can download them", !s.includes("not downloaded yet") && s.includes("27 other submodule entries have no address in .gitmodules"));
    ok("  and it says git cannot fetch them", s.includes("fetch them"));
    await clickInRow(page, "trinity", { text: "Show what changed" });
    await sleep(300);
    const s2 = await text(page);
    // Chrome's innerText applies CSS text-transform; that heading is uppercased.
    ok("expanding shows the commits it moved through", /commits it moved through/i.test(s2));
    ok("  with subjects and a tail count", s2.includes("Updated") && s2.includes("and 8 more"));
    await page.close();
  }
  {
    // A push refused for a submodule commit no remote has: one button pushes that submodule first and then this repository; another pushes anyway.
    const message = "trinity records d9b93c5, a commit none of trinity's remotes has yet — anyone fetching this push could not check it out. Push trinity first, or push anyway.";
    const refusal = {
      ok: false,
      headline: message,
      detail: "",
      refusal: { code: "submodules-unpushed", message },
      unpushed_submodules: [{ path: "trinity", recorded: "d9b93c5".padEnd(40, "0"), recorded_short: "d9b93c5", state: "not-on-remote", branch: "main" }],
    };
    const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER, pushResult: refusal });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    await (await button(page, "Push")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Push trinity first"), { timeout: 4000 });
    const s = await text(page);
    ok("a push refused for a submodule commit no remote has names the submodule and offers to push it first", s.includes("none of trinity's remotes has yet") && s.includes("Push trinity first"));
    ok("  and offers to push anyway", s.includes("Push anyway"));
    await (await button(page, "Push trinity first")).click();
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "changes_push").length >= 3, { timeout: 4000 });
    const pushes = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_push").map((c) => c.args));
    ok("  Push trinity first pushes the submodule, then this repository", pushes.length >= 3 && pushes[1].repoPath === "/repos/gitswitch/trinity" && pushes[2].repoPath === "/repos/gitswitch" && pushes[2].allowUnpushedSubmodules === false, JSON.stringify(pushes));
    await page.waitForFunction(() => document.body.innerText.includes("Push anyway"), { timeout: 4000 });
    await (await button(page, "Push anyway")).click();
    await page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "changes_push").length > n, { timeout: 4000 }, pushes.length);
    const last = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_push").at(-1).args);
    ok("  Push anyway sends the override, for this repository", last.repoPath === "/repos/gitswitch" && last.allowUnpushedSubmodules === true, JSON.stringify(last));
    await page.close();
  }
  {
    // The submodule's own refusal, chained from here, is shown as the submodule's: named, and without the repository's remedies.
    const message = "trinity records d9b93c5, which none of trinity's remote-tracking branches has — as far as this checkout knows. Anyone fetching this push could not check it out. Push trinity first (or fetch it, if that commit was pushed from elsewhere), or push anyway.";
    const refusal = { ok: false, headline: message, detail: "", refusal: { code: "submodules-unpushed", message }, unpushed_submodules: [{ path: "trinity", recorded: "d9b93c5".padEnd(40, "0"), recorded_short: "d9b93c5", state: "not-on-remote", branch: "fix" }] };
    const subRefusal = { ok: false, headline: "This branch tracks `origin/main`, which has another name. Publish it as `origin/fix` instead.", detail: "", refusal: { code: "upstream-name-differs", message: "This branch tracks `origin/main`, which has another name. Publish it as `origin/fix` instead." } };
    const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER, pushResult: refusal, subPushResult: subRefusal });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    await (await button(page, "Push")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Push trinity first"), { timeout: 4000 }).catch(() => null);
    await (await button(page, "Push trinity first"))?.click();
    await page.waitForFunction(() => document.body.innerText.includes("trinity: This branch tracks"), { timeout: 4000 }).catch(() => null);
    const s = await text(page);
    ok("a chained submodule push that is refused is shown as the submodule's, by name", s.includes("trinity: This branch tracks `origin/main`"));
    ok("  and the repository's own remedy for that code is not offered for it", (await button(page, "Publish as")) === null, JSON.stringify(await page.evaluate(() => [...document.querySelectorAll("button")].map((b) => b.innerText.trim()).filter((t) => t.startsWith("Publish")))));
    await page.close();
  }
  {
    // A detached submodule cannot push: the refusal offers to start a branch in it instead, and the card opens that row's form.
    const message = "trinity records d9b93c5, which none of trinity's remote-tracking branches has — as far as this checkout knows. Anyone fetching this push could not check it out. Push trinity first (it is detached — start a branch there first), or push anyway.";
    const refusal = { ok: false, headline: message, detail: "", refusal: { code: "submodules-unpushed", message }, unpushed_submodules: [{ path: "trinity", recorded: "d9b93c5".padEnd(40, "0"), recorded_short: "d9b93c5", state: "not-on-remote", branch: null }] };
    const detached = sub({ path: "trinity", name: "trinity", own_branch: null, own_upstream: null, own_unpushed: 1, recorded_on_remote: false, summary: "up to date with what this repo records; detached" });
    const page = await open(SCENARIOS.submodules, { submodules: [detached], subInner: SUB_INNER, pushResult: refusal });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    await (await button(page, "Push")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Start a branch in trinity first"), { timeout: 4000 }).catch(() => null);
    ok("for a detached submodule the refusal offers a branch there, not a push from it", (await button(page, "Start a branch in trinity first")) !== null && (await button(page, "Push trinity first")) === null);
    await (await button(page, "Start a branch in trinity first"))?.click();
    await page.waitForSelector('input[aria-label="Branch name for trinity"]', { timeout: 4000 }).catch(() => null);
    ok("  which opens that submodule's branch form on the card", (await page.$('input[aria-label="Branch name for trinity"]')) !== null);
    await page.close();
  }
  {
    // A detached submodule offers a branch at its checkout, named by the person; a recorded commit no remote has is marked on the row.
    const detached = sub({ path: "trinity", name: "trinity", own_branch: null, own_upstream: null, own_unpushed: 2, recorded_on_remote: false, summary: "up to date with what this repo records; detached" });
    // The switch leaves a submodule nested in trinity elsewhere: the result is trinity's.
    const createBranchTree = { ...TREE_RESET_OUTCOME, op: "create-branch", dropped: [], dropped_total: 0, backup: null, stash: null, recovery: null, submodule_mismatch: ["inner"] };
    const page = await open(SCENARIOS.submodules, { submodules: [detached], subInner: SUB_INNER, createBranchTree });
    await page.waitForFunction(() => document.body.innerText.includes("Start a branch here"), { timeout: 4000 });
    const s = await text(page);
    ok("a detached submodule's row offers Start a branch here", s.includes("Start a branch here"));
    ok("  and a recorded commit no remote has is marked on the row", s.includes("recorded commit not pushed"));
    await (await button(page, "Start a branch here")).click();
    await page.waitForSelector('input[aria-label="Branch name for trinity"]', { timeout: 4000 });
    await page.type('input[aria-label="Branch name for trinity"]', "fix/pointer");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_create_branch"), { timeout: 4000 });
    const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_create_branch").args);
    ok("  Enter creates the branch at the submodule's checkout and switches to it", call.repoPath === "/repos/gitswitch/trinity" && call.name === "fix/pointer" && call.from === null && call.switchTo === true, JSON.stringify(call));
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Align submodules"), { timeout: 4000 }).catch(() => null);
    await (await button(page, "Align submodules"))?.click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_submodule_align"), { timeout: 4000 }).catch(() => null);
    const al = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodule_align").at(-1)?.args ?? null);
    ok("  a submodule left elsewhere inside it is aligned there, not in the page's repository", al?.repoPath === "/repos/gitswitch/trinity" && JSON.stringify(al?.paths) === '["inner"]', JSON.stringify(al));
    await page.close();
  }
  {
    // While submodules download, the strip shows git's progress: phase, share, which submodule, bytes and rate.
    // A submodule that is listed in .gitmodules and not downloaded: the one Download missing fetches.
    const missing = sub({ path: "libs/sub", name: "sub", initialised: false, actual: null, actual_short: null, own_branch: null, own_upstream: null, state: "not-initialised", summary: "not downloaded yet — Download fetches it (git submodule update --init)" });
    const page = await open(SCENARIOS.submodules, {
      submodules: [SUBMODULES[0], missing],
      subInner: SUB_INNER,
      hang: true,
      cloneProgress: { phase: "Submodules", percent: 37, done: 3, total: 8, bytes: "10.00 MiB", rate: "2.00 MiB/s", detail: "libs/sub", estimate_kb: 1258291, eta_secs: 603 },
    });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 8000 }).catch(() => null);
    await page.waitForFunction(() => document.body.innerText.includes("Download missing"), { timeout: 8000 }).catch(() => null);
    const dl = await button(page, "Download missing");
    const dlState = dl ? await dl.evaluate((e) => ({ disabled: e.disabled, text: e.innerText.trim() })) : null;
    const listed = await page.evaluate(() => [...document.querySelectorAll('[aria-label="Submodules not downloaded yet"] li')].map((li) => li.innerText.replace(/\s+/g, " ").trim()));
    ok("the card names the submodule that is not downloaded yet", listed.length === 1 && listed[0].startsWith("libs/sub not downloaded"), JSON.stringify(listed));
    await dl?.click();
    await page.waitForFunction(() => document.body.innerText.includes("Submodules 37%"), { timeout: 8000 }).catch(() => null);
    const s = await text(page);
    ok("  (the Download button was there and enabled)", dlState !== null && dlState.disabled === false, JSON.stringify(dlState) + " | " + s.slice(s.indexOf("Submodules", s.indexOf("Submodules") + 1), s.indexOf("Submodules", s.indexOf("Submodules") + 1) + 500).replace(/\s+/g, " "));
    ok("while submodules download, the strip says which one and how far", s.includes("Downloading 1 submodule") && s.includes("Submodules 37%") && s.includes("3 of 8") && s.includes("libs/sub") && s.includes("2.00 MiB/s"), s.includes("Downloading") ? s.slice(s.indexOf("Downloading"), s.indexOf("Downloading") + 160) : "no 'Downloading' on the page: " + s.slice(0, 300).replace(/\s+/g, " "));
    ok("  with how much of the submodule's size has arrived, and how long is left at that rate", s.includes("10.00 MiB of about 1.2 GB") && s.includes("about 10 min left"), s.slice(s.indexOf("Submodules 37%"), s.indexOf("Submodules 37%") + 140));
    ok("  as a bar", await page.evaluate(() => !!document.querySelector('[aria-label="Download progress"]')));
    await page.close();
  }
  {
    // A download that was interrupted (a .git entry, nothing checked out) is not populated: it is offered for download again.
    const half = sub({ path: "half", name: "half", initialised: true, actual: null, actual_short: null, own_branch: null, own_upstream: null, state: "incomplete", summary: "its download was interrupted — nothing is checked out; Download finishes it (git submodule update --init)" });
    const page = await open(SCENARIOS.submodules, { submodules: [half, SUBMODULES[0]], subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    const s = await text(page);
    ok("an interrupted download is counted among the ones to download, not the populated", s.includes("Download missing (1)") && s.includes("1 not downloaded yet"), s.slice(s.indexOf("Submodules"), s.indexOf("Submodules") + 300).replace(/\s+/g, " "));
    ok("  and says what happened to it", s.includes("1 download interrupted — Download finishes it"), s.slice(s.indexOf("not downloaded"), s.indexOf("not downloaded") + 200).replace(/\s+/g, " "));
    const named = await page.evaluate(() => [...document.querySelectorAll('[aria-label="Submodules not downloaded yet"] li')].map((li) => li.innerText.replace(/\s+/g, " ").trim()));
    ok("  and names it, with its own Download", named.length === 1 && named[0].startsWith("half download interrupted") && named[0].endsWith("Download"), JSON.stringify(named));
    const title = await page.evaluate(() => [...document.querySelectorAll("button[title]")].map((b) => b.title).find((t) => t.startsWith("git submodule update --init for half ")) ?? "");
    ok("  Download missing's tooltip names what it fetches", title.includes("half"), title);
    await page.close();
  }
  {
    // Inside a submodule, the Submodules tab is about THAT submodule's own submodules — the only place they can be downloaded from.
    const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    const tab = (name) =>
      page.evaluate((n) => {
        const t = [...document.querySelectorAll('[role="tab"]')].find((b) => b.innerText.trim().startsWith(n));
        if (!t) return false;
        t.click();
        return true;
      }, name);
    ok("the Submodules tab opens", await tab("Submodules"));
    await page.waitForFunction(() => document.body.innerText.includes("Work inside"), { timeout: 4000 }).catch(() => null);
    const before = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodules").map((c) => c.args.repoPath));
    ok("  at the repository it reads the repository's submodules", before.length > 0 && before.every((p) => p === "/repos/gitswitch"), JSON.stringify(before));
    const inside = await page.evaluate(() => {
      const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes("trinity"));
      const b = row && [...row.querySelectorAll("button")].find((x) => x.innerText.trim() === "Work inside");
      if (!b) return false;
      b.click();
      return true;
    });
    ok("  Work inside scopes the page to the submodule", inside);
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 }).catch(() => null);
    ok("  and the Submodules tab opens again there", await tab("Submodules"));
    await page.waitForFunction(() => document.body.innerText.includes("Submodules of trinity"), { timeout: 4000 }).catch(() => null);
    const scoped = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodules").map((c) => c.args.repoPath).at(-1));
    ok("  where it reads the submodule's own submodules and says whose they are", scoped === "/repos/gitswitch/trinity", JSON.stringify(scoped));
    await page.close();
  }
  {
    // The card's rows: Record this pointer exactly where the checkout is ahead of the record.
    const lagging = sub({ path: "lagging", name: "lagging", ahead: 0, behind: 3, state: "moved", actual: "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0", actual_short: "a0a0a0a", summary: "sits 3 commits behind what this repo records; on main" });
    const page = await open(SCENARIOS.submodules, { submodules: [...SUBMODULES, lagging], subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    const rowButtons = (path) =>
      page.evaluate((p) => {
        const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes(p));
        return row ? [...row.querySelectorAll("button")].map((b) => b.innerText.trim()) : null;
      }, path);
    ok("the card's row for a submodule ahead of the record offers Record this pointer", (await rowButtons("trinity"))?.includes("Record this pointer") === true, JSON.stringify(await rowButtons("trinity")));
    ok("  one behind it offers Update to, not Record", (await rowButtons("lagging"))?.includes("Update to dceaf0b") === true && !(await rowButtons("lagging"))?.includes("Record this pointer"), JSON.stringify(await rowButtons("lagging")));
    ok("  one at the record offers neither", !(await rowButtons("delivery"))?.some((b) => b.startsWith("Update to") || b === "Record this pointer"), JSON.stringify(await rowButtons("delivery")));
    const subReads = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodules").length);
    const readsBefore = await subReads();
    ok("Record this pointer on trinity", await clickInRow(page, "trinity", { text: "Record this pointer" }));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_record_pointers"), { timeout: 4000 });
    const rec = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_record_pointers").at(-1).args);
    ok("  records that one path, in the repository", rec.repoPath === "/repos/gitswitch" && JSON.stringify(rec.paths) === '["trinity"]', JSON.stringify(rec));
    await page.waitForFunction(() => document.body.innerText.includes("Recorded 1 submodule pointer(s)."), { timeout: 4000 });
    ok("  the result card shows the commit it made, by its message", (await text(page)).includes("Record submodule pointers: trinity"));
    await page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "changes_submodules").length > n, { timeout: 4000 }, readsBefore);
    ok("  and the card re-reads", (await subReads()) > readsBefore, `${readsBefore} → ${await subReads()}`);
    await page.close();
  }
  {
    // A submodule on a branch of its own with no upstream: its commits rebased onto its remote's latest.
    const own = sub({ path: "solo", name: "solo", own_branch: "mine", own_upstream: null, latest_ref: "origin/main", summary: "on mine" });
    const page = await open(SCENARIOS.submodules, { submodules: [...SUBMODULES, own], subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    ok("a submodule on a branch without that upstream offers Rebase onto <remote>/<branch>", await clickInRow(page, "solo", { text: "Rebase onto origin/main" }));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_pull"), { timeout: 4000 }).catch(() => null);
    const pl = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_pull").at(-1)?.args ?? null);
    ok("  which rebases inside the submodule onto that branch", pl?.repoPath === "/repos/gitswitch/solo" && pl?.mode === "rebase" && pl?.source === "origin/main", JSON.stringify(pl));
    await page.close();
  }
  {
    // A checkout ahead of the record — a go-back left it there — can be put back on it.
    const lagging = sub({ path: "lagging", name: "lagging", ahead: 0, behind: 3, state: "moved", actual: "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0", actual_short: "a0a0a0a", summary: "sits 3 commits behind what this repo records; on main" });
    const page = await open(SCENARIOS.submodules, { submodules: [...SUBMODULES, lagging], subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    const rowButtons = (path) =>
      page.evaluate((p) => {
        const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes(p));
        return row ? [...row.querySelectorAll("button")].map((b) => b.innerText.trim()) : null;
      }, path);
    ok("the card's row ahead of the record also offers Check out <recorded>", (await rowButtons("trinity"))?.includes("Check out dceaf0b") === true, JSON.stringify(await rowButtons("trinity")));
    ok("  one behind it offers Update to instead", !(await rowButtons("lagging"))?.some((b) => b.startsWith("Check out")), JSON.stringify(await rowButtons("lagging")));
    ok("  one at the record offers neither", !(await rowButtons("delivery"))?.some((b) => b.startsWith("Check out")), JSON.stringify(await rowButtons("delivery")));
    ok("Check out on trinity", await clickInRow(page, "trinity", { text: "Check out dceaf0b" }));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_submodule_align"), { timeout: 4000 }).catch(() => null);
    const al = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodule_align").at(-1)?.args ?? null);
    ok("  checks out that one path, in the repository", al?.repoPath === "/repos/gitswitch" && JSON.stringify(al?.paths) === '["trinity"]', JSON.stringify(al));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("2000+"), { timeout: 4000 });
    await clickInRow(page, "staging", { text: "Show what changed" });
    await page.waitForFunction(() => document.body.innerText.includes("Changed inside"), { timeout: 4000 });
    ok("a dirty submodule lists its own changed files", (await text(page)).includes("inside/changed.ts"));
    const readOnly = await page.evaluate(() => {
      const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes("inside/changed.ts"));
      if (!row) return "row missing";
      const titles = [...row.querySelectorAll("button")].map((b) => b.title);
      return titles.some((x) => /Stage|Discard|Unstage/.test(x)) ? "has actions" : "read-only";
    });
    ok("  and offers no staging there (that's another repository)", readOnly === "read-only", readOnly);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
    await clickInRow(page, "staging", { title: "Work in this submodule as its own repository" });
    await sleep(500);
    const picked = await page.evaluate(() => JSON.parse(localStorage.getItem("gitswitch:changes.repo") ?? '""'));
    ok("Open switches the page to that submodule", picked.endsWith("/staging"), picked);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Submodules tab: each row's own operations");
  {
    // One submodule whose checkout sits behind what the repository records.
    const lagging = sub({ path: "lagging", name: "lagging", ahead: 0, behind: 3, state: "moved", actual: "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0", actual_short: "a0a0a0a", summary: "sits 3 commits behind what this repo records; on main" });
    const page = await open(SCENARIOS.submodules, { submodules: [...SUBMODULES, lagging], subInner: SUB_INNER });
    await page.waitForFunction(() => [...document.querySelectorAll('a[role="tab"]')].some((a) => a.innerText.trim().startsWith("Submodules")), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll('a[role="tab"]')].find((a) => a.innerText.trim().startsWith("Submodules")).click());
    await page.waitForFunction(() => document.body.innerText.includes("populated"), { timeout: 4000 });
    const rowButtons = (path) =>
      page.evaluate((p) => {
        const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes(p));
        return row ? [...row.querySelectorAll("button")].map((b) => b.innerText.trim()) : null;
      }, path);
    const clickInTr = (path, label) =>
      page.evaluate(
        (p, t) => {
          const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes(p));
          const b = row && [...row.querySelectorAll("button")].find((x) => x.innerText.trim().startsWith(t));
          if (!b) return false;
          b.click();
          return true;
        },
        path,
        label
      );
    const scope = () => page.evaluate(() => JSON.parse(localStorage.getItem("gitswitch:repo.sub") ?? '""'));
    ok("every row shows the submodule's own branch and upstream", (await text(page)).includes("main → origin/main"));
    const trinity = await rowButtons("trinity");
    ok("a submodule behind its own upstream offers Pull inside with the count", trinity?.includes("Pull inside (11)") === true, JSON.stringify(trinity));
    ok("  but not Update to: its checkout is ahead of the record", !trinity?.some((b) => b.startsWith("Update to")));
    ok("  nor Rebase onto: its branch already follows origin/main", !trinity?.some((b) => b.startsWith("Rebase onto")), JSON.stringify(trinity));
    ok("  so it offers Record this pointer instead", trinity?.includes("Record this pointer") === true, JSON.stringify(trinity));
    ok("  and Check out <recorded>, the way back to what the repository records", trinity?.includes("Check out dceaf0b") === true, JSON.stringify(trinity));
    const lag = await rowButtons("lagging");
    ok("a submodule behind the record offers Update to <recorded>", lag?.includes("Update to dceaf0b") === true, JSON.stringify(lag));
    ok("  and not Record this pointer: there is nothing ahead to record", !lag?.includes("Record this pointer"), JSON.stringify(lag));
    const delivery = await rowButtons("delivery");
    ok("a submodule at the recorded commit offers neither", !delivery?.some((b) => b.startsWith("Update to") || b === "Record this pointer"), JSON.stringify(delivery));
    const subReads = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodules").length);
    const readsBefore = await subReads();
    ok("Record this pointer on trinity", await clickInTr("trinity", "Record this pointer"));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_record_pointers"), { timeout: 4000 });
    const rec = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_record_pointers").at(-1).args);
    ok("  records that one path, in the repository (never inside the submodule)", rec.repoPath === "/repos/gitswitch" && JSON.stringify(rec.paths) === '["trinity"]', JSON.stringify(rec));
    await page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "changes_submodules").length > n, { timeout: 4000 }, readsBefore);
    ok("  then the table re-reads", (await subReads()) > readsBefore, `${readsBefore} → ${await subReads()}`);
    await page.evaluate(() => [...document.querySelectorAll('a[role="tab"]')].find((a) => a.innerText.trim().startsWith("Changes")).click());
    await page.waitForFunction(() => document.body.innerText.includes("Recorded 1 submodule pointer(s)."), { timeout: 4000 });
    ok("  and the result card on Changes shows the commit it made, by its message", (await text(page)).includes("Record submodule pointers: trinity"));
    await page.evaluate(() => [...document.querySelectorAll('a[role="tab"]')].find((a) => a.innerText.trim().startsWith("Submodules")).click());
    await page.waitForFunction(() => document.body.innerText.includes("populated"), { timeout: 4000 });
    ok("Pull inside on trinity", await clickInTr("trinity", "Pull inside"));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_pull"), { timeout: 4000 });
    const pull = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_pull").at(-1).args);
    ok("  pulls inside the submodule, on its own branch", pull.repoPath === "/repos/gitswitch/trinity", JSON.stringify(pull));
    ok("  with the page's LFS option — the parent's own use of LFS does not decide it", pull.withLfs === true, JSON.stringify(pull));
    ok("  and the scope stays the repository", (await scope()) === "");
    ok("Update to on lagging", await clickInTr("lagging", "Update to"));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_submodule_update"), { timeout: 4000 });
    const upd = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodule_update").at(-1).args);
    ok("  asks for that one path only, in the repository", upd.repoPath === "/repos/gitswitch" && JSON.stringify(upd.paths) === '["lagging"]', JSON.stringify(upd));
    // The row's buttons are disabled while the update above settles.
    await page.waitForFunction(() => {
      const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes("trinity"));
      const b = row && [...row.querySelectorAll("button")].find((x) => x.innerText.trim() === "Check out dceaf0b");
      return !!b && !b.disabled;
    }, { timeout: 4000 }).catch(() => null);
    ok("Check out on trinity", await clickInTr("trinity", "Check out dceaf0b"));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_submodule_align"), { timeout: 4000 }).catch(() => null);
    const al = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodule_align").at(-1)?.args ?? null);
    ok("  checks out that one path, in the repository (never inside the submodule)", al?.repoPath === "/repos/gitswitch" && JSON.stringify(al?.paths) === '["trinity"]', JSON.stringify(al));
    await page.waitForFunction(() => ![...document.querySelectorAll("button")].some((b) => b.disabled && b.innerText.trim() === "Work inside"), { timeout: 4000 }).catch(() => null);
    ok("Work inside on delivery", await clickInTr("delivery", "Work inside"));
    await sleep(400);
    ok("  makes it the scope and opens Changes", (await scope()) === "delivery" && (await page.evaluate(() => location.pathname)) === "/repo/changes", `${await scope()} ${await page.evaluate(() => location.pathname)}`);
    // With a submodule as the scope, the tab still previews the repository — and the drawer stays.
    await page.evaluate(() => [...document.querySelectorAll('a[role="tab"]')].find((a) => a.innerText.trim().startsWith("Submodules")).click());
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Preview update" && b.closest("span.ml-auto")), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Preview update" && b.closest("span.ml-auto")).click());
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_sync_plan"), { timeout: 4000 });
    await sleep(500);
    const dlg = await page.$('[role="dialog"][aria-label="Update"]');
    const planned = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_sync_plan").map((c) => c.args.repoPath));
    ok("Preview update from the tab, with a submodule as the scope, opens the drawer and keeps it open", dlg !== null);
    ok("  for the repository itself, the scope dropped", (await scope()) === "" && planned.length === 1 && planned[0] === "/repos/gitswitch", JSON.stringify(planned));
    // The drawer belongs to that repository: another scope takes it down.
    await page.evaluate(() => {
      const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes("delivery"));
      [...row.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Work inside")).click();
    });
    await page.waitForFunction(() => JSON.parse(localStorage.getItem("gitswitch:repo.sub") ?? '""') === "delivery", { timeout: 4000 });
    await sleep(300);
    ok("  and moving to another scope closes it", (await page.$('[role="dialog"][aria-label="Update"]')) === null);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Picking a repository from a long list");
  {
    const many = Array.from({ length: 30 }, (_, i) => ({
      path: `/repos/bulk-${i}`,
      name: i === 7 ? "argos-staging" : `project-${i}`,
      profile_id: "p1",
      profile_name: "Kirmada",
      profile_email: "me@personal.test",
    }));
    const page = await open(SCENARIOS.clean, { repos: [...REPOS, ...many] });
    await page.waitForFunction(() => document.body.innerText.includes("Repository"), { timeout: 4000 });
    // Open the Repository picker: the second combobox in the header.
    await page.evaluate(() => {
      const labels = [...document.querySelectorAll("label")];
      const repoLabel = labels.find((l) => l.innerText.trim() === "Repository");
      repoLabel.parentElement.querySelector("button").click();
    });
    await page.waitForFunction(() => !!document.querySelector('input[aria-label="Search repositories"]'), { timeout: 4000 });
    ok("the repository picker has a search box", true);
    const before = await page.evaluate(() =>
      [...document.querySelectorAll("button")].filter((b) => b.innerText.includes("project-")).length
    );
    ok("  and every repository is listed before typing", before > 20);

    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search repositories"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(i, "staging");
      i.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await sleep(200);
    const after = await page.evaluate(() => ({
      staging: [...document.querySelectorAll("button")].some((b) => b.innerText.includes("argos-staging")),
      others: [...document.querySelectorAll("button")].filter((b) => b.innerText.includes("project-")).length,
      typed: document.querySelector('input[aria-label="Search repositories"]').value,
    }));
    ok("  typing keeps what it typed", after.typed === "staging");
    ok("  narrows the list to the match", after.staging && after.others === 0);

    // Enter picks the highlighted row, so the list works without the mouse.
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search repositories"]');
      i.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    await sleep(250);
    const chosen = await page.evaluate(() => document.body.innerText.includes("argos-staging"));
    ok("  and Enter chooses it", chosen);
    ok("  the list closes behind it", !(await page.evaluate(() => !!document.querySelector('input[aria-label="Search repositories"]'))));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Repository"), { timeout: 4000 });
    await page.evaluate(() => {
      const labels = [...document.querySelectorAll("label")];
      labels.find((l) => l.innerText.trim() === "Repository").parentElement.querySelector("button").click();
    });
    await page.waitForFunction(() => !!document.querySelector('input[aria-label="Search repositories"]'), { timeout: 4000 });
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search repositories"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(i, "zzz-nothing");
      i.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await sleep(200);
    ok("a search with no match says so rather than looking empty",
      (await text(page)).includes("Nothing matches"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Account"), { timeout: 4000 });
    await page.evaluate(() => {
      const labels = [...document.querySelectorAll("label")];
      labels.find((l) => l.innerText.trim() === "Account").parentElement.querySelector("button").click();
    });
    await sleep(200);
    // Any search box at all, not just the repositories one: a short list
    // should be read at a glance, never typed into.
    ok("a short list like Account keeps no search box",
      !(await page.evaluate(() =>
        [...document.querySelectorAll("input")].some((i) =>
          (i.getAttribute("aria-label") || "").startsWith("Search")
        )
      )));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Every long dropdown searches and loads as it scrolls");
  {
    // The Account picker never asks for a search box: it gets one once the
    // list is long enough to need one. The page derives accounts from the
    // repositories, so distinct profiles are what make that list long.
    const manyAccountRepos = Array.from({ length: 20 }, (_, i) => ({
      path: `/repos/team-${i}`,
      name: `team-repo-${i}`,
      profile_id: `acct-${i}`,
      profile_name: i === 12 ? "Acme" : `Team ${i}`,
      profile_email: `me${i}@x.test`,
    }));
    const page = await open(SCENARIOS.clean, { repos: [...REPOS, ...manyAccountRepos] });
    await page.waitForFunction(() => document.body.innerText.includes("Account"), { timeout: 4000 });
    await page.evaluate(() => {
      const labels = [...document.querySelectorAll("label")];
      labels.find((l) => l.innerText.trim() === "Account").parentElement.querySelector("button").click();
    });
    await page.waitForFunction(() => !!document.querySelector('input[aria-label="Search"]'), { timeout: 4000 });
    ok("a long Account list gets a search box without being asked", true);
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(i, "acme");
      i.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await sleep(200);
    const narrowed = await page.evaluate(() => ({
      acme: [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Acme"),
      teams: [...document.querySelectorAll("button")].filter((b) => b.innerText.trim().startsWith("Team ")).length,
    }));
    ok("  and searching it works, ignoring case", narrowed.acme && narrowed.teams === 0);
    await page.close();
  }
  {
    const many = Array.from({ length: 300 }, (_, i) => ({
      path: `/repos/bulk-${i}`,
      name: `project-${String(i).padStart(3, "0")}`,
      profile_id: "p1",
      profile_name: "Kirmada",
      profile_email: "me@personal.test",
    }));
    const page = await open(SCENARIOS.clean, { repos: [...REPOS, ...many] });
    await page.waitForFunction(() => document.body.innerText.includes("Repository"), { timeout: 4000 });
    await page.evaluate(() => {
      const labels = [...document.querySelectorAll("label")];
      labels.find((l) => l.innerText.trim() === "Repository").parentElement.querySelector("button").click();
    });
    await page.waitForFunction(() => !!document.querySelector('input[aria-label="Search repositories"]'), { timeout: 4000 });
    const first = await page.evaluate(() => ({
      rows: [...document.querySelectorAll("button")].filter((b) => b.innerText.includes("project-")).length,
      note: document.body.innerText.includes("Showing 50 of 302"),
    }));
    ok("302 repositories render as a first page, not all at once", first.rows < 60);
    ok("  and it says how many are still below", first.note);

    // Scrolling to the bottom brings the next page in.
    await page.evaluate(() => {
      const list = [...document.querySelectorAll("div.max-h-64")].pop();
      list.scrollTop = list.scrollHeight;
      list.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await sleep(250);
    const second = await page.evaluate(() =>
      [...document.querySelectorAll("button")].filter((b) => b.innerText.includes("project-")).length
    );
    ok("  scrolling to the end loads more", second > first.rows);

    // Searching still reaches a repository far past the rendered window.
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search repositories"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(i, "project-287");
      i.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await sleep(250);
    const found = await page.evaluate(() => ({
      hit: [...document.querySelectorAll("button")].some((b) => b.innerText.includes("project-287")),
      rows: [...document.querySelectorAll("button")].filter((b) => b.innerText.includes("project-")).length,
      note: document.body.innerText.includes("Showing 50 of"),
    }));
    ok("  searching finds one far beyond what was rendered", found.hit && found.rows === 1);
    ok("  and the list stops saying there is more below", !found.note);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("The terminal drawer never outgrows the window");
  {
    // A remembered 900 px drawer (dragged tall on a big screen) in a 600 px
    // window: its header, pickers and close button were pushed above the
    // window's top edge, so it could neither be closed nor shrunk.
    const geo = (pg) =>
      pg.evaluate(() => {
        const d = document.querySelector('[aria-label="Terminal"]');
        const r = d?.getBoundingClientRect();
        const close = d?.querySelector('[aria-label="Close the terminal"]')?.getBoundingClientRect();
        return r ? { top: Math.round(r.top), height: Math.round(r.height), closeTop: Math.round(close?.top ?? -1), win: window.innerHeight } : null;
      });
    const page = await open(SCENARIOS.clean, { storage: { "console.height": 900 } });
    await page.setViewport({ width: 1280, height: 600 });
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal")?.click());
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    await sleep(300);
    let g = await geo(page);
    ok("a remembered height taller than the window is brought back inside it", !!g && g.top >= 0 && g.height <= g.win, JSON.stringify(g));
    ok("  so the close button is on screen", !!g && g.closeTop >= 0 && g.closeTop < g.win, JSON.stringify(g));
    // Dragging the handle to the very top cannot take the drawer past the window either.
    const handle = await page.$('[aria-label="Resize the terminal"]');
    const hb = await handle.boundingBox();
    await page.mouse.move(hb.x + hb.width / 2, hb.y + hb.height / 2);
    await page.mouse.down();
    await page.mouse.move(hb.x + hb.width / 2, 0, { steps: 5 });
    await page.mouse.move(hb.x + hb.width / 2, -50, { steps: 2 });
    await page.mouse.up();
    await sleep(200);
    g = await geo(page);
    ok("  nor can a drag to the top edge", !!g && g.top >= 0 && g.closeTop >= 0, JSON.stringify(g));
    // The window shrinks under an open drawer: it follows.
    await page.setViewport({ width: 1280, height: 450 });
    await sleep(300);
    g = await geo(page);
    ok("  and a window that shrinks under the open drawer brings it along", !!g && g.top >= 0 && g.closeTop >= 0 && g.height <= g.win, JSON.stringify(g));
    // Half / Full in the header: sized to this window, header still on screen.
    const clickLabel = (l) => page.evaluate((l) => document.querySelector(`[aria-label="${l}"]`)?.click(), l);
    await clickLabel("Full window");
    await sleep(250);
    g = await geo(page);
    ok("Full fills the window but for the page's header, with the drawer's header on screen", !!g && g.height >= g.win - 60 && g.height < g.win && g.closeTop >= 0, JSON.stringify(g));
    await clickLabel("Half the window");
    await sleep(250);
    g = await geo(page);
    ok("  Half is half of it", !!g && Math.abs(g.height - Math.floor(g.win / 2)) <= 2, JSON.stringify(g));
    // The note has its own close; the drawer's Close stays what it says.
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("What it guards"))?.click());
    await sleep(150);
    ok("What it guards opens its note", (await text(page)).includes("not a barrier"));
    await clickLabel("Close this note");
    await sleep(150);
    const t = await text(page);
    ok("  the note's own × closes the note, not the terminal", !t.includes("not a barrier") && !!(await page.$('[aria-label="Terminal"]:not(.hidden)')) && t.includes("What it guards"));
    await page.close();
  }

  section("The terminal drawer");
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    ok("the drawer is not in the page until it is asked for",
      !(await page.evaluate(() => !!document.querySelector('[aria-label="Terminal"]'))));

    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    ok("the sidebar button opens it", true);
    // xterm is code-split, so the shell starts once that chunk has arrived.
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "terminal_open"), { timeout: 8000 });
    ok("  it opens a shell for the selected repository",
      await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "terminal_open" && c.args?.repoPath === "/repos/gitswitch")));
    ok("  and asks for a size, so the shell wraps correctly",
      await page.evaluate(() => {
        const c = window.__CALLS__.find((x) => x.cmd === "terminal_open");
        return c?.args?.cols > 0 && c?.args?.rows > 0;
      }));
    ok("  it carries its own repository picker", await page.evaluate(() =>
      !!document.querySelector('[aria-label="Terminal"]')?.querySelector("button")));

    // The honest note about what the guards do and do not cover.
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.includes("What it guards")).click();
    });
    await sleep(200);
    const note = await text(page);
    ok("it says the shell uses the same account as your own terminal", note.includes("same account"));
    ok("  that git here refuses a force push and --no-verify", note.includes("force push") && note.includes("--no-verify"));
    ok("  and is honest that the guards can be stepped around", note.includes("not a barrier"));
    await page.close();
  }
  // The rows the terminal draws, the box they are drawn in, and the rows the
  // shell was last told it has. When the shell believes in more rows than are
  // drawn, a full-screen program paints below the visible edge and the last
  // lines cannot be reached — until something resizes the drawer.
  const termShape = (page) => page.evaluate(() => {
    const drawer = document.querySelector('[aria-label="Terminal"]');
    const rowsEl = drawer?.querySelector(".xterm-rows");
    const xterm = drawer?.querySelector(".xterm");
    if (!rowsEl || !xterm) return null;
    const host = xterm.parentElement.getBoundingClientRect();
    const rows = rowsEl.children.length;
    const cell = rows ? rowsEl.children[0].getBoundingClientRect().height : 0;
    const lastBottom = rows ? rowsEl.lastElementChild.getBoundingClientRect().bottom : 0;
    const sized = window.__CALLS__.filter((c) => c.cmd === "terminal_open" || c.cmd === "terminal_resize");
    const last = sized[sized.length - 1];
    return {
      rows, cell: Math.round(cell * 100) / 100, boxHeight: Math.round(host.height), lastBottom: Math.round(lastBottom), boxBottom: Math.round(host.bottom),
      windowHeight: window.innerHeight, toldRows: last?.args?.rows ?? null, toldBy: last?.cmd ?? null,
      opened: window.__CALLS__.some((c) => c.cmd === "terminal_open"),
    };
  });
  const openTerminal = async (page) => {
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"] .xterm-rows'), { timeout: 8000 });
  };
  {
    const page = await open(SCENARIOS.clean);
    await openTerminal(page);
    await sleep(1500);
    const s = await termShape(page);
    ok("every row the terminal draws is inside the drawer: the last one ends above its bottom edge",
      !!s && s.rows > 0 && s.lastBottom <= s.boxBottom + 1 && s.lastBottom <= s.windowHeight, JSON.stringify(s));
    ok("  and the box is used: one more row would not fit",
      !!s && s.cell > 0 && s.boxHeight - s.rows * s.cell < s.cell, JSON.stringify(s));
    ok("  the shell was told the same number of rows it is drawn with", !!s && s.toldRows === s.rows, JSON.stringify(s));
    // The box shrinks without the window changing: the note opens above it.
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.includes("What it guards")).click();
    });
    await sleep(900);
    const n = await termShape(page);
    ok("when the note opens above it the rows are fitted again, still inside the drawer",
      !!n && n.rows < s.rows && n.lastBottom <= n.boxBottom + 1, JSON.stringify(n));
    ok("  and the shell is told the new number", !!n && n.toldRows === n.rows && n.toldBy === "terminal_resize", JSON.stringify(n));
    await page.close();
  }
  {
    // A shell that takes a while to start: the box changes shape before the
    // session exists, so the resize has nowhere to go at that moment.
    const page = await open(SCENARIOS.clean, { terminalOpenDelayMs: 1800 });
    await openTerminal(page);
    await sleep(300);
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.includes("What it guards")).click();
    });
    await sleep(2600);
    const s = await termShape(page);
    ok("a shell that starts late is still told the rows the terminal ended up with",
      !!s && s.rows > 0 && s.toldRows === s.rows, JSON.stringify(s));
    ok("  which fit the drawer", !!s && s.lastBottom <= s.boxBottom + 1 && s.boxHeight - s.rows * s.cell < s.cell, JSON.stringify(s));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.keyboard.down("Control");
    await page.keyboard.press("`");
    await page.keyboard.up("Control");
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    ok("Ctrl+` opens it, the way every editor does", true);
    // Hidden, not unmounted: a shell must survive being closed.
    await page.keyboard.down("Control");
    await page.keyboard.press("`");
    await page.keyboard.up("Control");
    await sleep(250);
    const state = await page.evaluate(() => {
      const el = document.querySelector('[aria-label="Terminal"]');
      return { present: !!el, hidden: el ? el.className.includes("hidden") : false };
    });
    ok("  closing it hides the drawer rather than killing the shell", state.present && state.hidden);
    ok("  and the shell was not closed behind your back",
      !(await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "terminal_close"))));
    await page.close();
  }
  {
    // The drawer filters by account, the same way the Changes page does.
    const mixed = [
      { path: "/repos/work-a", name: "work-a", profile_id: "p2", profile_name: "Acme", profile_email: "me@work.test" },
      { path: "/repos/work-b", name: "work-b", profile_id: "p2", profile_name: "Acme", profile_email: "me@work.test" },
      { path: "/repos/mine-a", name: "mine-a", profile_id: "p1", profile_name: "Kirmada", profile_email: "me@personal.test" },
    ];
    const page = await open(SCENARIOS.clean, { repos: mixed });
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    await sleep(400);
    const drawerText = () => page.evaluate(() => document.querySelector('[aria-label="Terminal"]').innerText);
    ok("the drawer offers an account filter", (await drawerText()).includes("All accounts"));

    // Open the account picker (the first Select inside the drawer) and choose one.
    await page.evaluate(() => {
      const drawer = document.querySelector('[aria-label="Terminal"]');
      [...drawer.querySelectorAll("button")].find((b) => b.innerText.includes("All accounts")).click();
    });
    await sleep(200);
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Acme").click();
    });
    await sleep(400);
    ok("  choosing an account narrows the repositories to it", (await drawerText()).includes("work-"));
    ok("  and the shell follows the filter rather than staying somewhere hidden",
      await page.evaluate(() =>
        window.__CALLS__.filter((c) => c.cmd === "terminal_open").slice(-1)[0]?.args?.repoPath?.includes("/repos/work-")
      ));
    await page.close();
  }
  {
    // Two or three entries are a glance; a fourth is worth typing into.
    const four = [
      ...REPOS,
      { path: "/repos/c", name: "c", profile_id: "p3", profile_name: "Third", profile_email: "c@x.test" },
    ];
    const page = await open(SCENARIOS.clean, { repos: four });
    await page.waitForFunction(() => document.body.innerText.includes("Account"), { timeout: 4000 });
    await page.evaluate(() => {
      const labels = [...document.querySelectorAll("label")];
      labels.find((l) => l.innerText.trim() === "Account").parentElement.querySelector("button").click();
    });
    await sleep(250);
    ok("a four-entry Account list gets a search box",
      await page.evaluate(() =>
        [...document.querySelectorAll("input")].some((i) => (i.getAttribute("aria-label") || "").startsWith("Search"))
      ));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("The identity guard is switched from the commit box, on any repository");
  {
    const guardCalls = (page) => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "install_commit_guard" || c.cmd === "uninstall_commit_guard").map((c) => ({ cmd: c.cmd, ...c.args })));
    const clickText = (page, label) =>
      page.evaluate((l) => {
        const b = [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === l);
        if (!b) return false;
        b.click();
        return true;
      }, label);
    {
      const page = await open(SCENARIOS.clean);
      await page.waitForFunction(() => document.body.innerText.includes("Committing as"), { timeout: 4000 });
      ok("a repository with no wrong commits offers the guard", await clickText(page, "Turn on guard"));
      await sleep(400);
      ok("  which installs it with the folder profile's email", JSON.stringify(await guardCalls(page)) === JSON.stringify([{ cmd: "install_commit_guard", repoPath: "/repos/gitswitch", expectedEmail: "me@personal.test" }]), JSON.stringify(await guardCalls(page)));
      await page.close();
    }
    {
      const page = await open(SCENARIOS.guardOn);
      await page.waitForFunction(() => document.body.innerText.includes("Committing as"), { timeout: 4000 });
      ok("a guarded repository says so", (await text(page)).includes("guard on"));
      ok("  and turns it off again", await clickText(page, "Turn off guard"));
      await sleep(400);
      ok("  through the app's uninstall", JSON.stringify(await guardCalls(page)) === JSON.stringify([{ cmd: "uninstall_commit_guard", repoPath: "/repos/gitswitch" }]), JSON.stringify(await guardCalls(page)));
      await page.close();
    }
    {
      const page = await open(SCENARIOS.guardStale);
      await page.waitForFunction(() => document.body.innerText.includes("Committing as"), { timeout: 4000 });
      ok("a guard that expects another email says which", (await text(page)).includes("guard expects old@personal.test"));
      await page.evaluate(() => {
        const t = document.querySelector("textarea");
        const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value").set;
        setter.call(t, "a message");
        t.dispatchEvent(new Event("input", { bubbles: true }));
      });
      await sleep(250);
      const commit = await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Commit"))?.disabled);
      ok("  the commit waits for it, though the identity is right", commit === true && (await text(page)).includes("identity guard still expects old@personal.test"));
      ok("  Update guard is offered", await clickText(page, "Update guard"));
      await sleep(400);
      ok("  and rewrites it with the profile's email", JSON.stringify(await guardCalls(page)) === JSON.stringify([{ cmd: "install_commit_guard", repoPath: "/repos/gitswitch", expectedEmail: "me@personal.test" }]), JSON.stringify(await guardCalls(page)));
      await page.close();
    }
  }

  // -----------------------------------------------------------------
  section("The terminal opens where the page is");
  {
    const opens = (page) => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "terminal_open").map((c) => c.args?.repoPath));
    const closes = (page) => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "terminal_close").length);
    const waitOpens = (page, n) =>
      page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "terminal_open").length >= n, { timeout: 8000 }, n).then(() => true).catch(() => false);
    const sidebarTerminal = (page) => page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click());
    const ctrlBacktick = async (page) => {
      await page.keyboard.down("Control");
      await page.keyboard.press("`");
      await page.keyboard.up("Control");
      await sleep(300);
    };
    const pickOnPage = async (page, name) => {
      await page.evaluate(() => {
        const labels = [...document.querySelectorAll("label")];
        labels.find((l) => l.innerText.trim() === "Repository").parentElement.querySelector("button").click();
      });
      await sleep(250);
      const picked = await page.evaluate((n) => {
        const b = [...document.querySelectorAll(".absolute.z-50 button")].find((x) => x.innerText.trim().startsWith(n));
        if (!b) return false;
        b.click();
        return true;
      }, name);
      await sleep(400);
      return picked;
    };
    // The drawer's repository picker: the second Select inside the drawer.
    const drawerPicker = (page) =>
      page.evaluate(() => {
        const drawer = document.querySelector('[aria-label="Terminal"]');
        const triggers = [...drawer.querySelectorAll(".relative > button")];
        return triggers[1]?.innerText.trim() ?? null;
      });
    const openDrawerPicker = (page) =>
      page.evaluate(() => {
        const drawer = document.querySelector('[aria-label="Terminal"]');
        [...drawer.querySelectorAll(".relative > button")][1].click();
      });
    {
      const page = await open(SCENARIOS.clean);
      await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
      ok("(the page picks argos)", await pickOnPage(page, "argos"));
      await sidebarTerminal(page);
      await waitOpens(page, 1);
      ok("a repository picked on the page is where the sidebar's Terminal opens the shell", (await opens(page)).at(-1) === "/repos/argos", JSON.stringify(await opens(page)));
      await ctrlBacktick(page);
      ok("(the page picks gitswitch again)", await pickOnPage(page, "gitswitch"));
      await ctrlBacktick(page);
      await waitOpens(page, 2);
      ok("  Ctrl+` follows the page the same way", (await opens(page)).at(-1) === "/repos/gitswitch", JSON.stringify(await opens(page)));
      ok("  and the shell it leaves is closed, once", (await closes(page)) === 1, String(await closes(page)));
      await page.close();
    }
    {
      // The drawer's list was read before a clone was deleted: picking that
      // clone starts no shell there — the drawer goes back to the page's
      // folder and says why.
      const gone = { path: "/repos/gone-clone", name: "gone-clone", profile_id: "p2", profile_name: "Work", profile_email: "me@work.test" };
      const page = await open(SCENARIOS.clean, { repos: [...REPOS, gone], gonePaths: [gone.path] });
      await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
      await sidebarTerminal(page);
      await waitOpens(page, 1);
      await openDrawerPicker(page);
      await sleep(250);
      await page.evaluate(() => [...document.querySelectorAll(".absolute.z-50 button")].find((x) => x.innerText.trim().startsWith("gone-clone"))?.click());
      await sleep(1200);
      const s = await text(page);
      ok("a folder deleted since the drawer listed it gets no shell", !(await opens(page)).includes(gone.path), JSON.stringify(await opens(page)));
      ok("  the drawer goes back to the page's folder, and says why", (await opens(page)).at(-1) === "/repos/gitswitch" && s.includes("gone-clone is no longer on this Mac"), JSON.stringify(await opens(page)));
      await page.evaluate(() => document.querySelector('[aria-label="Close this note"]')?.click());
      await sleep(150);
      ok("  and that note can be closed, leaving the terminal open", !(await text(page)).includes("gone-clone is no longer on this Mac") && (await opens(page)).at(-1) === "/repos/gitswitch");
      ok("  and no error is left on screen", !s.includes("no longer exists on disk"));
      await page.close();
    }
    {
      // A shell that could not start says so on a line of its own, not over
      // the prompt, and the message goes once a shell starts elsewhere.
      const page = await open(SCENARIOS.clean, { terminalFails: ["/repos/argos"] });
      await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
      ok("(the page picks argos)", await pickOnPage(page, "argos"));
      await sidebarTerminal(page);
      await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"] [role="alert"]'), { timeout: 8000 }).catch(() => null);
      const geo = await page.evaluate(() => {
        const d = document.querySelector('[aria-label="Terminal"]');
        const alert = d?.querySelector('[role="alert"]')?.getBoundingClientRect();
        const xterm = d?.querySelector(".xterm")?.getBoundingClientRect();
        return alert && xterm ? { alertBottom: Math.round(alert.bottom), xtermTop: Math.round(xterm.top) } : null;
      });
      ok("a shell that could not start says so above the terminal, not over its first line", !!geo && geo.alertBottom <= geo.xtermTop + 1, JSON.stringify(geo));
      await openDrawerPicker(page);
      await sleep(250);
      await page.evaluate(() => [...document.querySelectorAll(".absolute.z-50 button")].find((x) => x.innerText.trim().startsWith("gitswitch"))?.click());
      await waitOpens(page, 2);
      await sleep(500);
      ok("  and the message goes once a shell starts in another folder", !(await page.evaluate(() => !!document.querySelector('[aria-label="Terminal"] [role="alert"]'))));
      await page.close();
    }
    {
      // The page itself remembered the deleted clone: it moves to a
      // repository that is there, and so does the terminal.
      const page = await open(SCENARIOS.clean, { storage: { "repo.path": "/repos/gone-clone", "changes.repo": "/repos/gone-clone", "console.repo": "/repos/gone-clone" }, gonePaths: ["/repos/gone-clone"] });
      await page.waitForFunction(() => JSON.parse(localStorage.getItem("gitswitch:repo.path") ?? '""') !== "/repos/gone-clone", { timeout: 6000 }).catch(() => null);
      const now = await page.evaluate(() => JSON.parse(localStorage.getItem("gitswitch:repo.path") ?? '""'));
      ok("a remembered repository deleted since gives way to one that is there", now === "/repos/gitswitch", now);
      await sidebarTerminal(page);
      await waitOpens(page, 1);
      await sleep(400);
      ok("  and the terminal opens there, never in the deleted folder", !(await opens(page)).includes("/repos/gone-clone") && (await opens(page)).at(-1) === "/repos/gitswitch", JSON.stringify(await opens(page)));
      await page.close();
    }
    {
      const page = await open(SCENARIOS.clean, { storage: { "console.repo": "/repos/argos" } });
      await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
      await sidebarTerminal(page);
      await waitOpens(page, 1);
      ok("a repository the drawer remembers from an earlier run does not win over the page's", (await opens(page))[0] === "/repos/gitswitch", JSON.stringify(await opens(page)));
      await page.close();
    }
    {
      const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER });
      await page.waitForFunction(() => document.body.innerText.includes("Submodules"), { timeout: 4000 });
      await page.evaluate(() => [...document.querySelectorAll('[role="tab"]')].find((b) => b.innerText.trim().startsWith("Submodules")).click());
      await page.waitForFunction(() => [...document.querySelectorAll("tr button")].some((b) => b.innerText.trim().startsWith("Work inside")), { timeout: 4000 });
      await page.evaluate(() => {
        const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes("delivery"));
        [...row.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Work inside")).click();
      });
      await page.waitForFunction(() => JSON.parse(localStorage.getItem("gitswitch:repo.sub") ?? '""') === "delivery", { timeout: 4000 });
      await sidebarTerminal(page);
      await waitOpens(page, 1);
      await sleep(600);
      ok("inside a submodule the shell opens in the submodule", (await opens(page)).at(-1) === "/repos/gitswitch/delivery", JSON.stringify(await opens(page)));
      ok("  is not swapped for another repository afterwards", (await opens(page)).length === 1, JSON.stringify(await opens(page)));
      const label = await drawerPicker(page);
      ok("  and the drawer's picker names it", (label ?? "").includes("delivery"), label);
      await page.close();
    }
    {
      const page = await open(SCENARIOS.submodules, {
        submodules: SUBMODULES,
        subInner: SUB_INNER,
        storage: { "repo.path": "/repos/gitswitch", "repo.sub": "delivery", "changes.repo": "/repos/gitswitch/delivery" },
      });
      await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
      await sidebarTerminal(page);
      await waitOpens(page, 1);
      await sleep(600);
      ok("  also when the page starts inside it", JSON.stringify(await opens(page)) === JSON.stringify(["/repos/gitswitch/delivery"]), JSON.stringify(await opens(page)));
      await page.close();
    }
    {
      const page = await open(SCENARIOS.clean);
      await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
      await ctrlBacktick(page);
      await waitOpens(page, 1);
      await ctrlBacktick(page);
      await page.evaluate((fresh) => { window.__REPOS__ = fresh; }, [
        ...REPOS,
        { path: "/repos/fresh", name: "fresh", profile_id: "p1", profile_name: "Kirmada", profile_email: "me@personal.test" },
      ]);
      await ctrlBacktick(page);
      await sleep(400);
      await openDrawerPicker(page);
      await sleep(250);
      const listed = await page.evaluate(() => [...document.querySelectorAll('[aria-label="Terminal"] .absolute.z-50 button')].map((b) => b.innerText.trim()));
      ok("a repository cloned since the drawer was last open is in its picker", listed.some((l) => l.startsWith("fresh")), JSON.stringify(listed));
      // Pick argos inside the drawer: the shell moves there.
      await page.evaluate(() => [...document.querySelectorAll('[aria-label="Terminal"] .absolute.z-50 button')].find((b) => b.innerText.trim().startsWith("argos")).click());
      await waitOpens(page, 2);
      await sleep(300);
      const [o, c] = [(await opens(page)).length, await closes(page)];
      await ctrlBacktick(page);
      await ctrlBacktick(page);
      await sleep(500);
      ok("a repository picked inside the drawer survives closing and reopening it while the page stays put",
        (await opens(page)).length === o && (await closes(page)) === c && (await opens(page)).at(-1) === "/repos/argos", JSON.stringify(await opens(page)));
      await page.close();
    }
    {
      const page = await open(SCENARIOS.clean);
      await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
      await sidebarTerminal(page);
      await waitOpens(page, 1);
      ok("(the page picks argos with the drawer open)", await pickOnPage(page, "argos"));
      const clicked = await page.evaluate(() => {
        const b = [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Terminal here");
        if (!b) return false;
        b.click();
        return true;
      });
      await waitOpens(page, 2);
      ok("the Repository page's Terminal here points an open drawer at the page's repository", clicked && (await opens(page)).at(-1) === "/repos/argos", JSON.stringify(await opens(page)));
      await page.close();
    }
  }

  // -----------------------------------------------------------------
  section("The drawer says whether this repository can be pushed");
  {
    const page = await open(SCENARIOS.clean, {
      pushState: { blocked: true, repo_blocked: true, reason: "Blocked for this repository (guard-rail)." },
    });
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    await page.waitForFunction(
      () => document.querySelector('[aria-label="Terminal"]').innerText.includes("Guard-rail"),
      { timeout: 4000 }
    );
    ok("a guard-railed repository is named as such before you type", true);
    ok("  and it is not called Locked, which means something stronger",
      !(await page.evaluate(() => document.querySelector('[aria-label="Terminal"]').innerText.includes("Locked"))));
    ok("  the reason is there to read", await page.evaluate(() =>
      !!document.querySelector('[aria-label="Terminal"] [title*="guard-rail"], [aria-label="Terminal"] [title*="Blocked"]')));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, {
      pushState: { blocked: true, repo_blocked: true, lock: { locked: true }, reason: "Locked for this repository. Unlocking needs the administrator password." },
    });
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(
      () => document.querySelector('[aria-label="Terminal"]')?.innerText.includes("Locked"),
      { timeout: 6000 }
    );
    ok("a locked repository is told apart from a guard-railed one", true);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    await sleep(700);
    const t = await page.evaluate(() => document.querySelector('[aria-label="Terminal"]').innerText);
    ok("a repository you can push to gets no badge at all", !t.includes("Guard-rail") && !t.includes("Locked"));
    // Push access changed on the page while the drawer is open: the badge follows.
    await clickSegment(page, "Guard-rail");
    const followed = await page.waitForFunction(() => document.querySelector('[aria-label="Terminal"]').innerText.includes("Guard-rail"), { timeout: 4000 }).then(() => true).catch(() => false);
    ok("  turning the guard-rail on in Push access shows it on the open drawer's badge at once", followed);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("The Activity tab: what the app itself ran");
  {
    const page = await open(SCENARIOS.clean, { consoleEntries: [
      { id: 1, at: "2026-09-30T10:00:00Z", repo: "/repos/gitswitch", argv: ["git", "-C", "/repos/gitswitch", "status", "--porcelain=v2"], exit_code: 0, duration_ms: 42, stdout: "", stderr: "", is_read: true, truncated: false },
      { id: 2, at: "2026-09-30T10:00:01Z", repo: "/repos/gitswitch", argv: ["git", "fetch", "--all"], exit_code: 0, duration_ms: 1400, stdout: "Fetching origin", stderr: "", is_read: false, truncated: false },
      { id: 3, at: "2026-09-30T10:00:02Z", repo: "/repos/gitswitch", argv: ["git", "push", "origin", "main"], exit_code: 128, duration_ms: 900, stdout: "", stderr: "fatal: refusing to push", is_read: false, truncated: false },
    ] });
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    await page.evaluate(() => document.querySelector('[aria-label="Activity tab"]').click());
    await page.waitForFunction(
      () => document.querySelector('[aria-label="Terminal"]').innerText.includes("git fetch --all"),
      { timeout: 4000 }
    );
    const drawer = () => page.evaluate(() => document.querySelector('[aria-label="Terminal"]').innerText);
    let d = await drawer();
    ok("it lists what the app ran", d.includes("git fetch --all"));
    ok("  with how long it took", d.includes("1.4s"));
    ok("  and names a command that failed by its exit code", d.includes("exit 128"));
    ok("  routine reads are hidden by default, because most of it is reads",
      !d.includes("--porcelain=v2"));

    await page.evaluate(() => document.querySelector('input[aria-label="Hide routine reads"]').click());
    await sleep(250);
    d = await drawer();
    ok("  and are shown when that is turned off", d.includes("--porcelain=v2"));

    // Output is behind a disclosure: a log that dumped everything is unreadable.
    ok("output is not dumped by default", !d.includes("refusing to push"));
    await page.evaluate(() => {
      const b = [...document.querySelectorAll('[aria-label^="Toggle git push"]')][0];
      b.click();
    });
    await sleep(250);
    ok("  and is there when a row is opened", (await drawer()).includes("refusing to push"));
    await page.close();
  }
  {
    // The one mistake that would kill a live shell.
    const page = await open(SCENARIOS.clean, { consoleEntries: [
      { id: 1, at: "2026-09-30T10:00:00Z", repo: "/repos/gitswitch", argv: ["git", "-C", "/repos/gitswitch", "status", "--porcelain=v2"], exit_code: 0, duration_ms: 42, stdout: "", stderr: "", is_read: true, truncated: false },
      { id: 2, at: "2026-09-30T10:00:01Z", repo: "/repos/gitswitch", argv: ["git", "fetch", "--all"], exit_code: 0, duration_ms: 1400, stdout: "Fetching origin", stderr: "", is_read: false, truncated: false },
      { id: 3, at: "2026-09-30T10:00:02Z", repo: "/repos/gitswitch", argv: ["git", "push", "origin", "main"], exit_code: 128, duration_ms: 900, stdout: "", stderr: "fatal: refusing to push", is_read: false, truncated: false },
    ] });
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "terminal_open"), { timeout: 8000 });
    await page.evaluate(() => document.querySelector('[aria-label="Activity tab"]').click());
    await sleep(300);
    await page.evaluate(() => document.querySelector('[aria-label="Shell tab"]').click());
    await sleep(300);
    ok("switching tabs does not close the shell",
      !(await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "terminal_close"))));
    ok("  and it is still the same session, not a second one",
      (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "terminal_open").length)) === 1);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { consoleEntries: [
      { id: 1, at: "2026-09-30T10:00:00Z", repo: "/repos/gitswitch", argv: ["git", "-C", "/repos/gitswitch", "status", "--porcelain=v2"], exit_code: 0, duration_ms: 42, stdout: "", stderr: "", is_read: true, truncated: false },
      { id: 2, at: "2026-09-30T10:00:01Z", repo: "/repos/gitswitch", argv: ["git", "fetch", "--all"], exit_code: 0, duration_ms: 1400, stdout: "Fetching origin", stderr: "", is_read: false, truncated: false },
      { id: 3, at: "2026-09-30T10:00:02Z", repo: "/repos/gitswitch", argv: ["git", "push", "origin", "main"], exit_code: 128, duration_ms: 900, stdout: "", stderr: "fatal: refusing to push", is_read: false, truncated: false },
    ] });
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Terminal").click();
    });
    await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 4000 });
    await page.evaluate(() => document.querySelector('[aria-label="Activity tab"]').click());
    await page.waitForFunction(
      () => document.querySelector('[aria-label="Terminal"]').innerText.includes("git fetch --all"),
      { timeout: 4000 }
    );
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Clear").click();
    });
    await sleep(300);
    ok("Clear empties the list and says what the tab is for",
      (await page.evaluate(() => document.querySelector('[aria-label="Terminal"]').innerText)).includes("Nothing yet"));
    ok("  and tells the backend to forget it too",
      await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "console_clear")));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Who would push: the account is shown before anything is sent");
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Pushes as"), { timeout: 4000 });
    const s = await text(page);
    ok("the card says which account a push would use", s.includes("Pushes as @octo-work"));
    ok("  and through which key", s.includes("key id_work"));
    ok("  with no warning when the profile does not say otherwise", !s.includes("would go as"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, {
      pushIdentity: { remote: "origin", url: "git@github.com:octo-home/x.git", via: "ssh-key", login: "octo-work", key_path: "/Users/me/.ssh/id_work", host: "github.com", account: null, expected_login: "octo-home", detail: "" },
    });
    await page.waitForFunction(() => document.body.innerText.includes("would go as"), { timeout: 4000 });
    const s = await text(page);
    ok("a push that would go as another account than the profile's is called out", s.includes("profile is @octo-home") && s.includes("would go as @octo-work"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, {
      pushIdentity: { remote: "origin", url: "https://github.com/acme/x.git", via: "https-credential", login: null, key_path: null, host: "github.com", account: null, expected_login: null, detail: "No credential is stored for github.com." },
    });
    await page.waitForFunction(() => document.body.innerText.includes("Can't tell who would push"), { timeout: 4000 });
    ok("when the account can't be told, the reason is shown", (await text(page)).includes("No credential is stored"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Git LFS: browsing the files and downloading part of them");
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    // The dialog opens before its listing arrives; wait for the listing.
    await page.waitForFunction(() => document.body.innerText.includes("in total"), { timeout: 4000 });
    const s = await text(page);
    ok("the browser opens with the totals in words", s.includes("5 files · 3.0 GB in total"));
    ok("  and says how much is still to come", s.includes("4 still to download (3.0 GB)"));
    ok("  folders are listed with what they would cost", s.includes("3 of 3 to download"));
    ok("  a file shows its own size", s.includes("1.0 MB"));
    ok("  and whether it is already here", s.includes("to download"));
    ok("  a downloaded-but-unwritten file is named as such", s.includes("downloaded, not written out"));
    ok("  Download selected starts disabled", await (await button(page, "Download selected")).evaluate((b) => b.disabled));

    // Pick one folder: the payload must be the folder, not its files.
    await page.evaluate(() => {
      const cb = document.querySelector('input[aria-label="Select folder media/video"]');
      cb.click();
    });
    await sleep(150);
    ok("selecting a folder counts its files and their weight",
      (await text(page)).includes("2 selected · 2 to download (3.0 GB)"));
    await (await button(page, "Download selected")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_lfs_pull_paths"), { timeout: 4000 });
    const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_lfs_pull_paths"));
    ok("  a fully chosen folder travels as the folder", JSON.stringify(call?.args?.paths) === JSON.stringify(["media/video"]));
    ok("  for the repository on screen", call?.args?.repoPath === "/repos/gitswitch");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 4000 });
    await page.evaluate(() => {
      document.querySelector('input[aria-label="Select file media/video/clip.bin"]').click();
    });
    await sleep(150);
    await (await button(page, "Download selected")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_lfs_pull_paths"), { timeout: 4000 });
    const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_lfs_pull_paths"));
    ok("one file out of a folder travels as that file", JSON.stringify(call?.args?.paths) === JSON.stringify(["media/video/clip.bin"]));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 4000 });
    ok("files already here are hidden by default", !(await text(page)).includes("model.bin"));
    await page.evaluate(() => {
      document.querySelector('input[aria-label="Only the ones still to download"]').click();
    });
    await sleep(200);
    ok("  and shown when that is turned off", (await text(page)).includes("model.bin"));
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search large files"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(i, "tone");
      i.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await sleep(250);
    const s = await text(page);
    ok("searching narrows the list to what matches", s.includes("tone.bin") && !s.includes("clip.bin"));
    ok("  and the folder holding it is still shown", s.includes("audio"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS, hang: true });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 4000 });
    await (await button(page, "Download everything")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Downloading LFS files"), { timeout: 4000 });
    ok("Download everything runs the whole-repository pull",
      await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "changes_lfs_pull")));
    ok("  and the browser closes behind it", !(await text(page)).includes("Only the ones still to download"));
    await page.close();
  }
  {
    // A long download must look like progress, not like a hung app.
    const page = await open(SCENARIOS.lfsRepo, {
      lfs: LFS_POINTERS,
      hang: true,
      lfsProgress: { file: "media/video/clip.bin", done: 2, total: 4, bytes: 1073741824, total_bytes: 3221225472 },
    });
    await page.waitForFunction(() => document.body.innerText.includes("Pull LFS files (5)"), { timeout: 4000 });
    await (await button(page, "Pull LFS files")).click();
    await page.waitForFunction(() => document.body.innerText.includes("2 of 4 files"), { timeout: 5000 });
    const s = await text(page);
    ok("the banner says how far the download has got", s.includes("2 of 4 files"));
    ok("  in bytes a person can read", s.includes("1.0 GB of 3.0 GB"));
    ok("  and names the file being transferred", s.includes("media/video/clip.bin"));
    // Coming back to the window while it downloads does not re-read the whole
    // folder (that competed with the download and made the app lag).
    const reads = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status").length);
    const before = await reads();
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await sleep(400);
    ok("  coming back to the window during the download does not re-read the folder", (await reads()) === before);
    await page.close();
  }
  {
    // Control: with nothing running, coming back does re-read it.
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS });
    await page.waitForFunction(() => document.body.innerText.includes("Pull LFS files (5)"), { timeout: 4000 });
    await sleep(4200); // past the focus refresh's minimum interval
    const before = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status").length);
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await sleep(400);
    ok("(control: with nothing running, coming back to the window re-reads the folder)", (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status").length)) > before);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_NOT_INSTALLED });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    ok("without git-lfs there is nothing to browse",
      await (await button(page, "Browse files…")).evaluate((b) => b.disabled));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 4000 });
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search large files"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(i, "nothing-matches-this");
      i.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await sleep(250);
    const s = await text(page);
    ok("a search with no match says so, naming what was searched for",
      s.includes('Nothing here matches "nothing-matches-this"'));
    ok("  and never claims everything is already downloaded",
      !s.includes("Every large file here is already downloaded"));
    await page.close();
  }
  {
    // An older git-lfs has no --json, so no sizes. Printing "0 B" would be a
    // measurement the app never made.
    const noSizes = {
      ...LFS_LISTING,
      sizes_known: false,
      total_bytes: 0,
      missing_bytes: 0,
      files: LFS_LISTING.files.map((f) => ({ ...f, size: 0 })),
      folders: LFS_LISTING.folders.map((f) => ({ ...f, bytes: 0, missing_bytes: 0 })),
    };
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS, lfsListing: noSizes });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 4000 });
    const s = await text(page);
    ok("unknown sizes are said to be unknown", s.includes("size unknown"));
    ok("  rather than printed as 0 B", !s.includes("0 B"));
    await page.close();
  }
  {
    // With the list capped, a folder still has to be downloadable whole —
    // otherwise its unlisted files can never be asked for at all.
    const capped = { ...LFS_LISTING, truncated: 3, total: 8 };
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS, lfsListing: capped });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 4000 });
    ok("the cap is stated", (await text(page)).includes("3 more files aren't listed here"));
    await page.evaluate(() => document.querySelector('input[aria-label="Select folder media"]').click());
    await sleep(150);
    await (await button(page, "Download selected")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_lfs_pull_paths"), { timeout: 4000 });
    const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_lfs_pull_paths"));
    ok("  and choosing the folder sends the folder, not just the rows on screen",
      JSON.stringify(call?.args?.paths) === JSON.stringify(["media"]));
    await page.close();
  }
  {
    // Progress belongs to downloads. During a push it would be the numbers a
    // previous download left behind.
    const page = await open(SCENARIOS.lfsRepo, {
      lfs: LFS_POINTERS,
      hang: true,
      lfsProgress: { file: "media/video/clip.bin", done: 2, total: 4, bytes: 1073741824, total_bytes: 3221225472 },
    });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Push")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Pushing"), { timeout: 4000 });
    await sleep(1400);
    ok("a push shows no large-file progress", !(await text(page)).includes("2 of 4 files"));
    await page.close();
  }
  {
    // Thousands of un-virtualised rows is the lag this feature exists to
    // avoid: the list is capped and says so.
    const many = Array.from({ length: 400 }, (_, i) => ({
      path: `bulk/file-${String(i).padStart(4, "0")}.bin`,
      dir: "bulk",
      size: 1024,
      present: false,
      downloaded: false,
    }));
    const big = {
      ...LFS_LISTING,
      files: many,
      folders: [{ path: "bulk", depth: 0, files: 400, missing: 400, bytes: 409600, missing_bytes: 409600 }],
      total: 400,
      present: 0,
      missing: 400,
      total_bytes: 409600,
      missing_bytes: 409600,
    };
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS, lfsListing: big });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 6000 });
    const s = await text(page);
    ok("a huge folder renders a capped list, and says so", s.includes("Showing the first 300 of 401 rows"));
    ok("  and the folder can still be chosen whole without opening it",
      s.includes("400 of 400 to download"));
    await page.evaluate(() => document.querySelector('input[aria-label="Select folder bulk"]').click());
    await sleep(200);
    await (await button(page, "Download selected")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_lfs_pull_paths"), { timeout: 4000 });
    const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_lfs_pull_paths"));
    ok("  choosing it sends one folder, not four hundred paths",
      JSON.stringify(call?.args?.paths) === JSON.stringify(["bulk"]));
    await page.close();
  }
  {
    // The browser is a wide table of paths and sizes: it has to survive a
    // narrow window like every other surface.
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS, width: 700 });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    await (await button(page, "Browse files…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Large files"), { timeout: 4000 });
    const over = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth + 1);
    ok("the browser doesn't overflow a 700px window", !over);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Repositories page: first repositories at once, the rest in the background; an account switch clears the list");
  {
    // The rest of the listing takes a while on purpose, so the "loading more" state is observable.
    const page = await openReposPage(browser, server, { restDelay: 1500 });
    await page.waitForFunction(() => document.body.innerText.includes("alice / a-1"), { timeout: 5000 });
    let s = await text(page);
    ok("the first page paints as soon as the first 10 arrive", s.includes("alice / a-1") && s.includes("alice / a-10"));
    ok("  and says more are loading", s.includes("loading more…") && s.includes("signed in as alice"));
    const calls0 = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "list_remote_repos_page").map((c) => [c.args.page, c.args.perPage]));
    ok("  the first request asked for 10, the next for 100", JSON.stringify(calls0[0]) === "[1,10]" && JSON.stringify(calls0[1]) === "[1,100]");
    await page.waitForFunction(() => !document.body.innerText.includes("loading more…"), { timeout: 5000 });
    s = await text(page);
    ok("once the rest arrived the count is complete", s.includes("of 30") && s.includes("alice / a-1"));
    ok("  with no duplicates from the overlapping first page", (s.match(/alice \/ a-3\b/g) || []).length === 1);
    // Switch to bob, whose first page takes a moment: alice's list must not linger.
    await (await button(page, "alice")).click();
    await sleep(100);
    await (await button(page, "bob")).click();
    await sleep(150);
    s = await text(page);
    ok("switching accounts clears the other account's list at once", !s.includes("alice / a-1") && !s.includes("signed in as alice"));
    ok("  and shows the loading placeholder in the list area", s.includes("Loading every repository this account can reach"));
    await page.waitForFunction(() => document.body.innerText.includes("bob / b-1"), { timeout: 5000 });
    s = await text(page);
    ok("  then bob's repositories appear, signed in as bob", s.includes("bob / b-1") && s.includes("signed in as bob") && !s.includes("alice / a-1"));
    await page.close();
  }
  {
    // Leaving the page and coming back while the rest is still loading: the
    // visit joins the load already running and paints what has arrived.
    const page = await openReposPage(browser, server, { restDelay: 3000 });
    await page.waitForFunction(() => document.body.innerText.includes("alice / a-1"), { timeout: 5000 });
    const nav = (name) => page.evaluate((n) => [...document.querySelectorAll("a")].find((a) => a.textContent.trim() === n).click(), name);
    await nav("Profiles");
    await sleep(200);
    await nav("Repositories");
    await sleep(300);
    let s = await text(page);
    ok("coming back while the rest still loads shows the first repositories at once", s.includes("alice / a-1") && s.includes("alice / a-10"), s.slice(0, 300));
    ok("  and says more are loading, not the empty-list spinner", s.includes("loading more…") && !s.includes("Loading every repository this account can reach"));
    ok("  without asking GitHub again", (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "list_remote_repos_page" && c.args.perPage === 10).length)) === 1);
    await page.waitForFunction(() => !document.body.innerText.includes("loading more…"), { timeout: 6000 });
    s = await text(page);
    ok("  then the rest arrives on this visit too", s.includes("of 30"));
    await page.close();
  }

  section("Clone page: progress after the main download");
  {
    const page = await openClonePage(browser, server, {
      cloneHang: true,
      cloneProgress: { phase: "Submodules", percent: 37, done: 3, total: 8, bytes: null, rate: null, detail: "libs/sub" },
    });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Clone").click());
    await page.waitForFunction(() => document.body.innerText.includes("Submodules 37%"), { timeout: 4000 });
    const s = await text(page);
    ok("while submodules download, the line says which one it is on", s.includes("Submodules 37%") && s.includes("3 of 8") && s.includes("libs/sub"), s.slice(s.indexOf("Submodules"), s.indexOf("Submodules") + 60));
    ok("  with no size and no time left claimed while nothing says either", !s.includes("of about") && !s.includes("left"));
    await page.close();
  }
  {
    // A submodule in flight: what its host says it weighs, what has arrived, and the time left.
    const page = await openClonePage(browser, server, {
      cloneHang: true,
      cloneProgress: { phase: "Submodules", percent: 40, done: 3, total: 8, bytes: "10.00 MiB", rate: "2.00 MiB/s", detail: "libs/sub", estimate_kb: 1258291, eta_secs: 603 },
    });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Clone").click());
    await page.waitForFunction(() => document.body.innerText.includes("Submodules 40%"), { timeout: 4000 });
    const s = await text(page);
    ok("a submodule's download shows how much has arrived of its own size, the rate, and how long is left",
      s.includes("libs/sub") && s.includes("10.00 MiB of about 1.2 GB") && s.includes("2.00 MiB/s") && s.includes("about 10 min left"),
      s.slice(s.indexOf("Submodules 40%"), s.indexOf("Submodules 40%") + 160));
    await page.close();
  }
  {
    // A host that does not say how big the repository is: the time left still shows, the size does not.
    const page = await openClonePage(browser, server, {
      cloneHang: true,
      cloneProgress: { phase: "Submodules", percent: 40, done: 3, total: 8, bytes: "10.00 MiB", rate: "2.00 MiB/s", detail: "libs/sub", eta_secs: 4200 },
    });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Clone").click());
    await page.waitForFunction(() => document.body.innerText.includes("Submodules 40%"), { timeout: 4000 });
    const s = await text(page);
    ok("on a host that does not say the size, the time left still shows and no size is invented",
      s.includes("about 1 h 10 min left") && !s.includes("of about"), s.slice(s.indexOf("Submodules 40%"), s.indexOf("Submodules 40%") + 160));
    await page.close();
  }
  {
    const page = await openClonePage(browser, server, {
      cloneHang: true,
      cloneProgress: { phase: "Large files", percent: 40, done: 2, total: 5, bytes: "1.0 MiB of 4.0 MiB", rate: null, detail: "media/video/intro.mp4" },
    });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Clone").click());
    await page.waitForFunction(() => document.body.innerText.includes("Large files 40%"), { timeout: 4000 });
    const s = await text(page);
    ok("while large files download, the line counts them, with the bytes git-lfs reported and the file it is on", s.includes("2 of 5") && s.includes("1.0 MiB of 4.0 MiB") && s.includes("media/video/intro.mp4") && !s.includes("of about"), s.slice(s.indexOf("Large files"), s.indexOf("Large files") + 90));
    await page.close();
  }

  section("Edit Profile shows the profile's own GitHub account");
  {
    const home = { id: "p1", name: "Home", git_name: "Some Display Name", git_email: "me@home.test", ssh_key_path: null, github_login: "octo-home", is_default: true, allow_push: true, signing_enabled: false, directories: [], remote_patterns: [], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
    const picked = (page) => page.evaluate(() => {
      const card = [...document.querySelectorAll("div")].find((d) => d.innerText.toLowerCase().startsWith("autofill from github"));
      return (card?.querySelector("button")?.innerText ?? "").trim();
    });
    for (const [label, profile, keyAccount] of [
      ["Edit Profile shows the account the profile records, though its git username is a display name", home, null],
      ["  even when its key cannot be probed", { ...home, ssh_key_path: "/k/id_home" }, null],
      ["  however the stored login is spelled", { ...home, github_login: "@Octo-Home" }, null],
    ]) {
      const page = await openProfilePage(browser, server, { path: "/profile/p1/edit", profiles: [profile], ghAccounts: ["octo-work", "octo-home"], keyAccount });
      await page.waitForFunction(() => document.body.innerText.toLowerCase().includes("autofill from github"), { timeout: 5000 }).catch(() => null);
      await page.waitForFunction(() => {
        const card = [...document.querySelectorAll("div")].find((d) => d.innerText.toLowerCase().startsWith("autofill from github"));
        return (card?.querySelector("button")?.innerText ?? "").includes("octo-home");
      }, { timeout: 4000 }).catch(() => null);
      ok(label, (await picked(page)).includes("octo-home"), await picked(page));
      if (profile === home) {
        ok("  the identity fields are left as saved", await page.evaluate(() => document.querySelector('input[placeholder="Your git user.name"]')?.value) === "Some Display Name");
      }
      await page.close();
    }
  }

  section("Profiles: HTTPS clones push over SSH with the profile's key");
  {
    const work = { id: "p2", name: "Work", git_name: "octo-work", git_email: "me@work.test", ssh_key_path: "/k/id_work", github_login: "octo-work", is_default: false, allow_push: true, signing_enabled: false, directories: ["/w"], remote_patterns: [], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
    const box = (pg) =>
      pg.evaluate(() => {
        const i = [...document.querySelectorAll("label")].find((x) => x.innerText.includes("Push HTTPS clones over SSH"))?.querySelector('input[type="checkbox"]');
        return i ? JSON.stringify({ checked: i.checked, disabled: i.disabled }) : "missing";
      });
    const page = await openProfilePage(browser, server, { path: "/profile/p2/edit", profiles: [work] });
    await page.waitForFunction(() => document.body.innerText.includes("Push HTTPS clones over SSH"), { timeout: 5000 }).catch(() => null);
    ok("a profile with a key offers to push its HTTPS clones over SSH, off until turned on", (await box(page)) === JSON.stringify({ checked: false, disabled: false }), await box(page));
    await page.evaluate(() => [...document.querySelectorAll("label")].find((x) => x.innerText.includes("Push HTTPS clones over SSH"))?.querySelector('input[type="checkbox"]')?.click());
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Update Profile")?.click());
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "update_profile"), { timeout: 4000 }).catch(() => null);
    const sent = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "update_profile")?.args ?? null);
    ok("  turning it on is saved as on", sent?.pushOverSsh === true, JSON.stringify(sent));
    await page.close();
    const nokey = await openProfilePage(browser, server, { path: "/profile/p2/edit", profiles: [{ ...work, ssh_key_path: null }] });
    await nokey.waitForFunction(() => document.body.innerText.includes("Push HTTPS clones over SSH"), { timeout: 5000 }).catch(() => null);
    ok("  without a key it cannot be on", (await box(nokey)) === JSON.stringify({ checked: false, disabled: true }), await box(nokey));
    await nokey.close();
  }

  // -----------------------------------------------------------------
  section("GitHub CLI accounts: removing one");
  {
    const clickText = (page, label) =>
      page.evaluate((l) => {
        const b = [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === l);
        if (!b) return false;
        b.click();
        return true;
      }, label);
    const openRemove = (page) => page.evaluate(() => document.querySelector('button[title="Remove this account from the GitHub CLI"]')?.click());
    {
      const page = await openAuthPage(browser, server, { ghAccounts: ["alice", "bob"], logoutError: "Command execution error: gh logout failed for alice: no accounts matched that criteria" });
      await page.waitForFunction(() => document.body.innerText.includes("alice"), { timeout: 4000 });
      await openRemove(page);
      await sleep(200);
      await clickText(page, "Remove Account");
      await sleep(400);
      const s = await text(page);
      ok("a failed removal says why, inside the dialog", s.includes("Remove GitHub Account?") && s.includes("no accounts matched that criteria"), s.slice(0, 300));
      ok("  and Remove Account can be pressed again", await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Remove Account")?.disabled === false));
      await clickText(page, "Cancel");
      await sleep(200);
      await openRemove(page);
      await sleep(200);
      ok("  reopening the dialog starts clean", !(await text(page)).includes("no accounts matched"));
      await page.close();
    }
    {
      const page = await openAuthPage(browser, server, { ghAccounts: ["alice", "bob"] });
      await page.waitForFunction(() => document.body.innerText.includes("alice"), { timeout: 4000 });
      await openRemove(page);
      await sleep(200);
      await clickText(page, "Remove Account");
      await sleep(500);
      const s = await text(page);
      ok("a successful removal closes the dialog", !s.includes("Remove GitHub Account?"));
      ok("  and the list is read again, without the account", (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "gh_list_accounts").length)) >= 2 && !s.includes("alice") && s.includes("bob"));
      await page.close();
    }
  }

  section("GitHub CLI accounts: making another one active");
  {
    // The row of one login: the box holding its login and its buttons.
    const rowOf = (page, login) =>
      page.evaluate((l) => {
        const span = [...document.querySelectorAll("span.font-mono")].find((s) => s.textContent.trim().startsWith(l) && s.closest('[class*="rounded-lg"]')?.innerText.includes("Use this account"));
        return span ? span.closest('[class*="rounded-lg"]').innerText.replace(/\s+/g, " ") : "";
      }, login);
    const makeActive = (page, login) => page.evaluate((l) => document.querySelector(`button[title^="gh auth switch --hostname github.com --user ${l}"]`)?.click(), login);
    {
      const page = await openAuthPage(browser, server, { ghAccounts: ["alice", "bob"], ghActive: "alice" });
      await page.waitForFunction(() => document.body.innerText.includes("Make active"), { timeout: 4000 });
      ok("the login gh uses carries the Active badge and no switch", (await rowOf(page, "alice")).includes("Active") && !(await rowOf(page, "alice")).includes("Make active"), await rowOf(page, "alice"));
      ok("  the other login offers Make active", (await rowOf(page, "bob")).includes("Make active") && !(await rowOf(page, "bob")).includes("Active "), await rowOf(page, "bob"));
      ok("  and the card says what the active account is for", (await text(page)).includes("GitSwitch always says which account it is using"));
      await makeActive(page, "bob");
      await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "gh_auth_switch"), { timeout: 4000 });
      const asked = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "gh_auth_switch")?.args ?? null);
      ok("clicking it asks gh for that login on its host", JSON.stringify(asked) === JSON.stringify({ host: "github.com", login: "bob" }), JSON.stringify(asked));
      await page.waitForFunction(() => document.body.innerText.includes("is now the GitHub CLI's active account"), { timeout: 4000 }).catch(() => null);
      await sleep(300);
      ok("  the badge moves to it", (await rowOf(page, "bob")).includes("Active") && !(await rowOf(page, "alice")).includes("Active"), `${await rowOf(page, "alice")} | ${await rowOf(page, "bob")}`);
      ok("  and gh's own sentence is shown", (await text(page)).includes("bob is now the GitHub CLI's active account on github.com"));
      await page.close();
    }
    {
      const page = await openAuthPage(browser, server, { ghAccounts: ["alice", "bob"], ghActive: "alice", switchError: "Command execution error: gh could not make bob active on github.com: boom" });
      await page.waitForFunction(() => document.body.innerText.includes("Make active"), { timeout: 4000 });
      await makeActive(page, "bob");
      await page.waitForFunction(() => document.body.innerText.includes("gh could not make bob active"), { timeout: 4000 }).catch(() => null);
      const s = await text(page);
      ok("a switch gh refuses says why under the card", s.includes("gh could not make bob active on github.com: boom") && !s.includes("Command execution error"), s.slice(0, 200));
      ok("  and the badge stays where it was", (await rowOf(page, "alice")).includes("Active") && (await rowOf(page, "bob")).includes("Make active"));
      await page.close();
    }
    {
      const page = await openAuthPage(browser, server, { ghAccounts: ["alice", "bob"] });
      await page.waitForFunction(() => document.body.innerText.includes("Use this account"), { timeout: 4000 });
      const s = await text(page);
      ok("a gh that does not say which account is active shows neither badge nor switch", !s.includes("Make active") && !s.includes("Active") && s.includes("alice") && s.includes("bob"));
      await page.close();
    }
  }

  section("GitHub sign-in: a way out of every wait");
  {
    const signIn = (page) => page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Sign in with GitHub")?.click());
    const polls = (page) => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "github_poll_token").length);
    {
      const page = await openAuthPage(browser, server, { codeHold: true });
      await signIn(page);
      await page.waitForFunction(() => document.body.innerText.includes("Requesting a sign-in code"), { timeout: 4000 });
      ok("while a sign-in code is requested, the page offers Cancel", await page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Cancel")));
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Cancel")?.click());
      await sleep(300);
      const s = await text(page);
      ok("  Cancel there returns to both sign-in choices", s.includes("GitHub Device Authorization") && s.includes("Use GitHub CLI"));
      await page.evaluate(() => window.__HELD__.code?.res());
      await sleep(2500);
      ok("  a code that arrives after Cancel is neither shown nor polled for", !(await text(page)).includes("ABCD-1234") && (await polls(page)) === 0);
      await page.close();
    }
    {
      const page = await openAuthPage(browser, server, { codeHold: true });
      await signIn(page);
      await page.waitForFunction(() => document.body.innerText.includes("Requesting a sign-in code"), { timeout: 4000 });
      await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim().startsWith("Profiles"))?.click());
      await sleep(300);
      await page.evaluate(() => window.__HELD__.code?.res());
      await sleep(2500);
      ok("a code that arrives after the page is left starts no polling", (await polls(page)) === 0);
      await page.close();
    }
  }

  section("GitHub sign-in reaches Connected! through failures that are not GitHub's answer");
  {
    const user = { id: 4242, login: "octo-signin", name: "Octo Signin", email: null, avatar_url: "" };
    const page = await openAuthPage(browser, server, {
      polls: [
        { reject: "authorization_pending" },
        { reject: "Failed to poll for token: error sending request for url (https://github.com/login/oauth/access_token): operation timed out" },
        { reject: "GitHub returned 502 Bad Gateway: " },
        { reject: "slow_down" },
        { resolve: { access_token: "gho_test", token_type: "bearer", scope: "read:user" } },
      ],
      verifies: [{ reject: "HTTP request failed: connection reset" }, { resolve: user }],
    });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Sign in with GitHub")?.click());
    const stillTrying = await page.waitForFunction(() => document.body.innerText.includes("still trying"), { timeout: 8000 }).then(() => true).catch(() => false);
    ok("a check GitHub did not answer keeps the code on screen, and the page says it is still trying", stillTrying && (await text(page)).includes("ABCD-1234"));
    const connected = await page.waitForFunction(() => document.body.innerText.includes("Connected!"), { timeout: 30000 }).then(() => true).catch(() => false);
    const calls = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "github_poll_token" || c.cmd === "verify_github_token").map((c) => ({ cmd: c.cmd, at: c.at, token: c.args?.token })));
    const pollsAt = calls.filter((c) => c.cmd === "github_poll_token").map((c) => c.at);
    const verifies = calls.filter((c) => c.cmd === "verify_github_token");
    ok("the device-flow sign-in reaches Connected! with the account's login", connected && (await text(page)).includes("@octo-signin"));
    ok("  a timed-out check or a 502 while waiting does not end the sign-in", pollsAt.length === 5, String(pollsAt.length));
    ok("  after slow_down the next check waits at least 5 s longer", pollsAt.length === 5 && pollsAt[4] - pollsAt[3] >= 5500, String(pollsAt[4] - pollsAt[3]));
    ok("  a token GitHub granted is checked again, not polled for again", verifies.length === 2 && verifies.every((v) => v.token === "gho_test") && pollsAt.length === 5);
    await page.close();
  }

  section("A GitHub sign-in fills a profile later, as a gh login does");
  {
    const user = { id: 4242, login: "octo-signin", name: "Octo Signin", email: null, avatar_url: "" };
    const page = await openAuthPage(browser, server, {
      keepStorage: true,
      kept: ["octo-signin"],
      polls: [{ resolve: { access_token: "gho_test", token_type: "bearer", scope: "read:user" } }],
      verifies: [{ resolve: user }],
    });
    const field = (label) =>
      page.evaluate((l) => [...document.querySelectorAll("label")].find((x) => x.innerText.trim() === l)?.parentElement.querySelector("input")?.value ?? null, label);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Sign in with GitHub")?.click());
    await page.waitForFunction(() => document.body.innerText.includes("Connected!"), { timeout: 8000 });
    ok("the sign-in itself is kept — through the backend, with the token GitHub granted", await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "github_signin_keep" && c.args?.token === "gho_test")));
    ok("  and the Connected screen says what that gives", (await text(page)).includes("keeps the sign-in in this computer's keychain"));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Create Profile from GitHub")?.click());
    await page.waitForFunction(() => [...document.querySelectorAll("label")].some((l) => l.innerText.trim() === "Git Email"), { timeout: 4000 });
    await sleep(300);
    ok("Create Profile from GitHub fills the login and the no-reply email, as the form's autofill does",
      (await field("Git Username")) === "octo-signin" && (await field("Git Email")) === "4242+octo-signin@users.noreply.github.com",
      `${await field("Git Username")} / ${await field("Git Email")}`);
    ok("  the account is preselected in Autofill from GitHub", (await text(page)).includes("octo-signin · signed in with GitHub"));
    await page.goto(`${server.url}/profile/new`, { waitUntil: "networkidle0" });
    await page.waitForFunction(() => document.body.innerText.toLowerCase().includes("autofill from github"), { timeout: 4000 }).catch(() => null);
    ok("with no gh login, the profile form offers Autofill from the account signed in with GitHub", (await text(page)).toLowerCase().includes("autofill from github"));
    await page.evaluate(() => {
      const card = [...document.querySelectorAll("div")].find((d) => d.innerText.toLowerCase().startsWith("autofill from github"));
      card?.querySelector("button")?.click();
    });
    await sleep(250);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("octo-signin · signed in with GitHub") && b.closest(".absolute"))?.click());
    await sleep(500);
    ok("  picking it fills the same login and email", (await field("Git Username")) === "octo-signin" && (await field("Git Email")) === "4242+octo-signin@users.noreply.github.com");
    ok("  without asking gh for a token", !(await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "gh_get_token"))));
    await page.goto(`${server.url}/github`, { waitUntil: "networkidle0" });
    await page.waitForFunction(() => document.body.innerText.includes("Signed in with GitHub on this computer"), { timeout: 4000 }).catch(() => null);
    ok("the sign-in is listed on the GitHub page after a reload", (await text(page)).includes("Signed in with GitHub on this computer") && (await text(page)).includes("octo-signin"));
    ok("  as kept in the keychain", (await text(page)).includes("Sign-in kept in the keychain"));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Forget")?.click());
    await sleep(300);
    ok("  Forget forgets the kept sign-in as well", await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "github_signin_forget" && c.args?.login === "octo-signin")));
    await page.goto(`${server.url}/profile/new`, { waitUntil: "networkidle0" });
    await sleep(400);
    ok("  and once forgotten, the form no longer offers it", !(await text(page)).toLowerCase().includes("autofill from github"));
    await page.close();
  }

  {
    // A credential store that refuses: the sign-in still connects, and says
    // what is missing without it.
    const user = { id: 4242, login: "octo-signin", name: "Octo Signin", email: null, avatar_url: "" };
    const page = await openAuthPage(browser, server, {
      keepError: "Command execution error: The system's credential store refused it: no keychain",
      polls: [{ resolve: { access_token: "gho_test", token_type: "bearer", scope: "read:user" } }],
      verifies: [{ resolve: user }],
    });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Sign in with GitHub")?.click());
    await page.waitForFunction(() => document.body.innerText.includes("Connected!"), { timeout: 8000 }).catch(() => null);
    const s = await text(page);
    ok("a sign-in the keychain refused still connects, and says repositories and keys need gh for it", s.includes("Connected!") && s.includes("could not be kept on this computer") && s.includes("no keychain"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Autofill fills the account's own git identity");
  {
    const scoped = { id: 42, login: "octo-home", name: "Octo Home", email: null, avatar_url: "", emails: ["me@home.test", "alt@home.test"], primary_email: "me@home.test", email_scope_missing: false };
    const unscoped = { ...scoped, emails: [], primary_email: null, email_scope_missing: true };
    const pick = async (page, login) => {
      await page.waitForFunction(() => document.body.innerText.toLowerCase().includes("autofill from github"), { timeout: 5000 }).catch(() => null);
      await page.evaluate(() => {
        const card = [...document.querySelectorAll("div")].find((d) => d.innerText.toLowerCase().startsWith("autofill from github"));
        card?.querySelector("button")?.click();
      });
      await sleep(250);
      await page.evaluate((l) => [...document.querySelectorAll(".absolute button")].find((b) => b.innerText.trim() === l)?.click(), login);
      await sleep(600);
    };
    const value = (page, placeholder) => page.evaluate((ph) => document.querySelector(`input[placeholder="${ph}"]`)?.value ?? null, placeholder);
    {
      const page = await openProfilePage(browser, server, { path: "/profile/new", ghAccounts: ["octo-home"], user: scoped });
      await pick(page, "octo-home");
      ok("the git username is the login, not the display name", (await value(page, "Your git user.name")) === "octo-home");
      ok("the git email is the account's primary address, not the no-reply one", (await value(page, "your@email.com")) === "me@home.test", await value(page, "your@email.com"));
      const t = await text(page);
      ok("  the no-reply and the other verified address are one click away", t.includes("42+octo-home@users.noreply.github.com") && t.includes("alt@home.test"));
      await page.close();
    }
    {
      const page = await openProfilePage(browser, server, { path: "/profile/new", ghAccounts: ["octo-home"], user: unscoped });
      await pick(page, "octo-home");
      ok("without user:email the page says how to share the private addresses", (await text(page)).includes("gh auth refresh -h github.com -s user:email"));
      await page.close();
    }
    {
      const stored = { id: "p1", name: "Home", git_name: "octo-home", git_email: "me@home.test", ssh_key_path: null, is_default: true, allow_push: true, signing_enabled: false, directories: [], remote_patterns: [], github_login: "octo-home", created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
      const page = await openProfilePage(browser, server, { path: "/profile/p1/edit", profiles: [stored], ghAccounts: ["octo-home"], user: unscoped });
      await page.waitForFunction(() => document.querySelector('input[placeholder="your@email.com"]')?.value === "me@home.test", { timeout: 5000 }).catch(() => null);
      await pick(page, "octo-home");
      ok("re-picking a profile's own account keeps its stored address when the token cannot read the account's", (await value(page, "your@email.com")) === "me@home.test", await value(page, "your@email.com"));
      await page.close();
    }
  }

  // -----------------------------------------------------------------
  section("SSH Keys: a key made on an earlier visit can be registered from its row");
  {
    const clickText = (page, label) =>
      page.evaluate((l) => {
        const b = [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === l);
        if (!b) return false;
        b.click();
        return true;
      }, label);
    const registerFromRow = async (page) => {
      await clickText(page, "Register");
      await sleep(250);
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("Select an account…"))?.click());
      await sleep(250);
      await page.evaluate(() => [...document.querySelectorAll(".absolute button")].find((b) => b.innerText.trim() === "alice")?.click());
      await sleep(200);
      // The row's own Register button, inside the expanded row.
      await page.evaluate(() => {
        const buttons = [...document.querySelectorAll("button")].filter((b) => b.innerText.trim() === "Register" && !b.disabled);
        buttons[buttons.length - 1]?.click();
      });
      await sleep(600);
    };
    {
      const page = await openSshPage(browser, server);
      await registerFromRow(page);
      const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "gh_register_ssh_key")?.args ?? null);
      ok("a key from an earlier visit is registered from its row", call?.keyPath === "/home/me/.ssh/id_ed25519_gitswitch_work" && call?.account === "alice", JSON.stringify(call));
      ok("  and the row says which account it authenticates as", (await text(page)).includes("authenticates as") && (await text(page)).includes("alice"));
      await page.close();
    }
    {
      // A sign-in kept here registers keys as well as a gh login does.
      const page = await openSshPage(browser, server, { kept: ["octo-kept"], register: { resolve: "Registered on octo-kept" } });
      await clickText(page, "Register");
      await sleep(250);
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("Select an account…"))?.click());
      await sleep(250);
      const offered = await page.evaluate(() => [...document.querySelectorAll(".absolute button")].map((b) => b.innerText.trim()));
      ok("a sign-in kept here is offered for registering a key, beside the gh logins", offered.includes("octo-kept (signed in here)") && offered.includes("alice"), JSON.stringify(offered));
      await page.evaluate(() => [...document.querySelectorAll(".absolute button")].find((b) => b.innerText.trim() === "octo-kept (signed in here)")?.click());
      await sleep(200);
      await page.evaluate(() => {
        const buttons = [...document.querySelectorAll("button")].filter((b) => b.innerText.trim() === "Register" && !b.disabled);
        buttons[buttons.length - 1]?.click();
      });
      await sleep(600);
      const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "gh_register_ssh_key")?.args ?? null);
      ok("  and registering with it names that sign-in", call?.account === "signin:octo-kept", JSON.stringify(call));
      await page.close();
    }
    {
      const page = await openSshPage(browser, server, { register: { reject: "This gh login can't add SSH keys to alice. Grant it once with: gh auth refresh -h github.com -s admin:public_key, then press Register again." } });
      await registerFromRow(page);
      ok("a login without the key scope is told the command that fixes it", (await text(page)).includes("gh auth refresh"));
      await page.close();
    }
    {
      const page = await openSshPage(browser, server, { noClipboard: true });
      await clickText(page, "Copy public key");
      await sleep(400);
      ok("a copy that fails says so", (await text(page)).includes("Couldn't copy"));
      await page.close();
    }
    {
      const page = await openSshPage(browser, server);
      await page.type('input[placeholder="e.g., work, personal"]', "new");
      await page.type('input[placeholder="your@email.com"]', "me@example.test");
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Generate Key"))?.click());
      await page.waitForFunction(() => document.body.innerText.includes("Next: Test Connection"), { timeout: 4000 }).catch(() => null);
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("Next: Test Connection"))?.click());
      await page.waitForFunction(() => document.body.innerText.includes("Test SSH Connection"), { timeout: 4000 }).catch(() => null);
      ok("the Test step has a way back", await clickText(page, "Back"));
      await sleep(250);
      ok("  to the key", (await text(page)).includes("Your Public Key"));
      await page.close();
    }
  }

  section("Auto Assign: a profile without a usable key is matched by its saved account");
  {
    const page = await openAutoAssignPage(browser, server, { width: 900 });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Scan")?.click());
    await page.waitForFunction(() => !!document.querySelector('[data-autoassign-found="2"]'), { timeout: 8000 }).catch(() => null);
    await sleep(300);
    const t = await text(page);
    ok("a profile without an SSH key is proposed through its saved GitHub account", t.includes("Bot (acme-bot · saved account)"), t.slice(t.indexOf("repos found"), t.indexOf("repos found") + 300));
    ok("  and the reason shows at 900 px", await page.evaluate(() => [...document.querySelectorAll("main span, main div")].some((el) => el.innerText.trim().startsWith("push · acme-bot") && el.getBoundingClientRect().width > 0)));
    ok("a repository no account can see says why", t.includes("no account has access"));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Apply 1 assignment"))?.click());
    await sleep(500);
    const upd = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "update_profile").map((c) => c.args));
    ok("Apply assigns it to that profile", upd.some((a) => a.id === "pk" && (a.directories ?? []).includes("/w/widgets")), JSON.stringify(upd));
    await page.close();
  }

  section("Auto Assign: where it searches, each repository's path, and the ones already added labelled with their profile and account");
  {
    const page = await openAutoAssignPage(browser, server, { width: 1100, hold: true });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Scan")?.click());
    await page.waitForFunction(() => (document.querySelector("[data-autoassign-progress]")?.innerText ?? "").includes("1,234 folders"), { timeout: 4000 }).catch(() => null);
    const prog = await page.evaluate(() => document.querySelector("[data-autoassign-progress]")?.innerText ?? "");
    ok("while it walks it says where: the folder, how many folders, how many found", prog.includes("Searching /w · 1,234 folders · 1 found"), prog);
    ok("  and the folder it is in now, below the one searched", (await page.evaluate(() => document.querySelector("[data-autoassign-place]")?.innerText ?? "")) === "in client/deep/folder");
    const scanArgs = (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "scan_repos_tracked").map((c) => c.args)))[0] ?? {};
    ok("  the scan is asked for that folder, under an id progress is polled with", scanArgs.root === "/w" && !!scanArgs.scanId && (await page.evaluate((id) => window.__CALLS__.some((c) => c.cmd === "find_repos_progress" && c.args.scanId === id), scanArgs.scanId)), JSON.stringify(scanArgs));
    await (await button(page, "Stop"))?.click();
    await sleep(200);
    ok("Stop cancels that scan, and says nothing was changed", (await page.evaluate((id) => window.__CALLS__.some((c) => c.cmd === "find_repos_cancel" && c.args.scanId === id), scanArgs.scanId)) && (await text(page)).includes("Stopped. Nothing was changed"));
    await page.evaluate(() => window.__RELEASE_SCAN__?.());
    await sleep(400);
    ok("  what the stopped scan answers later is dropped", !(await page.$("[data-autoassign-found]")) && (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "get_profiles").length)) === 0);
    await page.close();

    const REPOS = [
      { path: "/w/widgets", remote_url: "git@github.com:acme/widgets.git", owner: "acme", name: "widgets" },
      { path: "/w/secret", remote_url: "git@github.com:acme/secret.git", owner: "acme", name: "secret" },
      { path: "/w/team/tool", remote_url: "git@github.com:acme/tool.git", owner: "acme", name: "tool" },
      { path: "/w/moved", remote_url: "git@github.com:acme/moved.git", owner: "acme", name: "moved" },
      { path: "/w/local", remote_url: "", owner: "", name: "" },
    ];
    const full = await openAutoAssignPage(browser, server, {
      width: 1100,
      repos: REPOS,
      dirs: { p1: ["/w/secret"], pk: ["/w/widgets", "/w/moved"] },
      push: { "acme/widgets": ["acme-bot"], "acme/tool": ["me-personal"], "acme/moved": ["me-personal"] },
    });
    await full.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Scan")?.click());
    await full.waitForFunction(() => !!document.querySelector('[data-autoassign-found="5"]'), { timeout: 8000 }).catch(() => null);
    const head = await full.evaluate(() => document.querySelector("[data-autoassign-found]")?.innerText ?? "");
    ok("the results say how many and in which folder", head.endsWith("in /w") && /5 repos found/i.test(head), head);
    ok("  and what the search passed over", (await text(full)).includes("Searched 42 folders, up to five levels deep."));
    const row = (p) => full.evaluate((p) => document.querySelector(`[data-autoassign-row="${p}"]`)?.innerText.replace(/\s+/g, " ") ?? "", p);
    const path = (p) => full.evaluate((p) => document.querySelector(`[data-autoassign-row="${p}"] [data-autoassign-path]`)?.innerText ?? "", p);
    ok("each repository shows its whole path, the folder searched dimmed", (await path("/w/team/tool")) === "/w/team/tool" && (await path("/w/widgets")) === "/w/widgets" && (await full.evaluate(() => document.querySelector('[data-autoassign-row="/w/team/tool"] [data-autoassign-path] .text-zinc-400')?.innerText)) === "team/tool");
    ok("  with the whole path on hover", (await full.evaluate(() => document.querySelector('[data-autoassign-row="/w/team/tool"] [data-autoassign-path]')?.getAttribute("title"))) === "/w/team/tool");
    ok("one already in the right profile: 'Already in Bot (acme-bot)', and the check", (await row("/w/widgets")).includes("Already in Bot (acme-bot)") && !!(await full.$('[data-autoassign-row="/w/widgets"] [aria-label="already assigned correctly"]')), await row("/w/widgets"));
    ok("one in a profile that nothing can be checked for: 'In Personal (me-personal)'", (await row("/w/secret")).includes("In Personal (me-personal)") && !(await row("/w/secret")).includes("Already in"), await row("/w/secret"));
    ok("one in a profile whose account cannot push to it: says where it is and where Apply moves it", (await row("/w/moved")).includes("In Bot (acme-bot) now → Apply moves it to Personal (me-personal)"), await row("/w/moved"));
    ok("  one in no profile has no such label", !(await row("/w/team/tool")).includes("Already in") && !(await row("/w/team/tool")).includes(" now →"));
    const chip = (k) => full.evaluate((k) => document.querySelector(`[data-autoassign-chip="${k}"]`)?.innerText ?? "", k);
    ok("the chips count: to assign, already assigned, not assignable", (await chip("all")) === "All 5" && (await chip("assign")) === "To assign 2" && (await chip("assigned")) === "Already assigned 2" && (await chip("none")) === "Not assignable 1", [await chip("all"), await chip("assign"), await chip("assigned"), await chip("none")].join(" | "));
    await full.evaluate(() => document.querySelector('[data-autoassign-chip="assigned"]')?.click());
    await sleep(150);
    ok("  a chip shows only its rows", JSON.stringify(await full.evaluate(() => [...document.querySelectorAll("[data-autoassign-row]")].map((r) => r.dataset.autoassignRow))) === JSON.stringify(["/w/widgets", "/w/secret"]));
    await full.evaluate(() => document.querySelector('[data-autoassign-chip="all"]')?.click());
    await sleep(150);
    await full.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Apply 2 assignments"))?.click());
    await sleep(500);
    const upd = await full.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "update_profile").map((c) => c.args));
    ok("Apply is as before: the two to assign go to the profile whose account can push", upd.length === 1 && upd[0].id === "p1" && JSON.stringify(upd[0].directories) === JSON.stringify(["/w/secret", "/w/team/tool", "/w/moved"]), JSON.stringify(upd));
    await full.close();
  }

  section("Leaving a page and coming back keeps where you were");
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Changes"), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll('a[role="tab"]')].find((a) => a.innerText.trim().startsWith("History"))?.click());
    await sleep(400);
    await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim().startsWith("Doctor"))?.click());
    await sleep(400);
    await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim().startsWith("Repository"))?.click());
    await sleep(500);
    ok("coming back to Repository opens the tab you left, History — not Changes", (await page.evaluate(() => location.pathname)) === "/repo/history");
    await page.close();
  }
  {
    const WORK = { id: "p2", name: "Work", git_name: "Me At Work", git_email: "me@work.test", ssh_key_path: "/Users/me/.ssh/id_work", is_default: false, allow_push: true, signing_enabled: false, directories: ["/work"], remote_patterns: [], github_login: null, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
    const page = await openClonePage(browser, server, { profiles: [WORK] });
    await page.evaluate(() => {
      const label = [...document.querySelectorAll("label")].find((l) => l.innerText.trim() === "Clone as");
      label?.parentElement.querySelector("button")?.click();
    });
    await sleep(250);
    await page.evaluate(() => [...document.querySelectorAll(".absolute button")].find((b) => b.innerText.trim().startsWith("Work"))?.click());
    await sleep(200);
    await page.type('input[placeholder="Defaults to the repo name"]', "data-work");
    await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim().startsWith("Doctor"))?.click());
    await sleep(400);
    await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim().startsWith("Clone"))?.click());
    await sleep(500);
    const asLabel = await page.evaluate(() => [...document.querySelectorAll("label")].find((l) => l.innerText.trim() === "Clone as")?.parentElement.querySelector("button")?.innerText.trim() ?? "");
    const folder = await page.evaluate(() => document.querySelector('input[placeholder="Defaults to the repo name"]')?.value ?? "");
    ok("Clone as and the folder name are what you chose, after visiting another page", asLabel.startsWith("Work") && folder === "data-work", `${asLabel} / ${folder}`);
    await page.close();
  }

  section("Doctor: the GitHub access check");
  {
    const page = await open(SCENARIOS.clean);
    await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim().startsWith("Doctor"))?.click());
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Check now"), { timeout: 4000 }).catch(() => null);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Check now")?.click());
    await page.waitForFunction(() => document.body.innerText.includes("@octo cannot see acme/app"), { timeout: 4000 }).catch(() => null);
    ok("Doctor's access check lists what it found", (await text(page)).includes("@octo cannot see acme/app"));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Run checks")?.click());
    await sleep(700);
    ok("  a re-run of the other checks keeps it", (await text(page)).includes("@octo cannot see acme/app"));
    ok("  and the access check can be run again", (await page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Check again"))) === true);
    await page.close();
  }

  section("Ports: who holds a port, and stopping it");
  {
    const page = await openPortsPage(browser, server);
    await page.waitForFunction(() => document.body.innerText.includes("of 5"), { timeout: 4000 }).catch(() => null);
    let s = await text(page);
    ok("a listener from one of my repositories shows its clone and profile", s.includes("gitswitch · Kirmada") && s.includes("node · pid 725"));
    ok("  the toggle defaults to my folders when any row has one, hiding the rest", s.includes("1 of 5") && !s.includes("mongod"), s.slice(s.indexOf("Only my"), s.indexOf("Only my") + 60));
    await page.evaluate(() => document.querySelector('[aria-label="Only my repositories\' folders"]')?.click());
    await sleep(200);
    s = await text(page);
    const stopOf = (name) => page.evaluate((n) => {
      const li = [...document.querySelectorAll('li[role="row"]')].find((l) => l.innerText.includes(n));
      const b = li && [...li.querySelectorAll("button")].find((x) => x.innerText.trim() === "Stop");
      return b ? { disabled: b.disabled, title: b.title } : null;
    }, name);
    ok("  unticked, every port is listed — a root process with its Stop disabled and the reason under the row", s.includes("5 of 5") && (await stopOf("launchd"))?.disabled === true && (await stopOf("launchd"))?.title.startsWith("Owned by root") && s.includes("sudo kill 1"), JSON.stringify(await stopOf("launchd")));
    await page.type('[aria-label="Filter ports"]', "27017");
    await sleep(250);
    s = await text(page);
    ok("  the filter narrows by port", s.includes("1 of 5") && s.includes("mongod") && !s.includes("launchd"));
    // A controlled input: cleared the way a person would, so React sees it.
    await page.click('[aria-label="Filter ports"]', { clickCount: 3 });
    await page.keyboard.press("Backspace");
    await page.waitForFunction(() => document.body.innerText.includes("5 of 5"), { timeout: 4000 }).catch(() => null);
    // Stop asks first, naming the process, the port and where it came from.
    const clickStopIn = (needle) => page.evaluate((n) => {
      const li = [...document.querySelectorAll('li[role="row"]')].find((l) => l.innerText.includes(n));
      const b = li && [...li.querySelectorAll("button")].find((x) => x.innerText.trim() === "Stop");
      if (!b) return false;
      b.click();
      return true;
    }, needle);
    ok("  (the node row is back once the filter is cleared)", await clickStopIn("node · pid 725"));
    await sleep(250);
    s = await text(page);
    ok("Stop asks first, naming the process, the port and the clone it came from", s.includes("Stop node (pid 725) listening on tcp 18789?") && s.includes("It was started from gitswitch."));
    ok("  and nothing has run yet", !(await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "ports_kill"))));
    const listsBefore = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "ports_list").length);
    await page.evaluate(() => [...document.querySelectorAll('[role="dialog"] button, .fixed button')].find((b) => b.innerText.trim() === "Stop")?.click());
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "ports_kill"), { timeout: 4000 }).catch(() => null);
    await sleep(300);
    const kill = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "ports_kill")?.args ?? null);
    ok("  confirming stops it by pid and name, and the list is read again", kill?.pid === 725 && kill?.force === false && kill?.name === "node" && (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "ports_list").length)) > listsBefore, JSON.stringify(kill));
    // A Docker-published port is never pid-killed: the container is stopped.
    const docker = await page.evaluate(() => {
      const li = [...document.querySelectorAll('li[role="row"]')].find((l) => l.innerText.includes("carpool-proxy-1"));
      return li ? [...li.querySelectorAll("button")].map((b) => ({ t: b.innerText.trim(), d: b.disabled })) : null;
    });
    ok("a port published by Docker offers Stop container (and the project), never an enabled Stop", !!docker && docker.some((b) => b.t === "Stop container" && !b.d) && docker.some((b) => b.t.startsWith("Stop project carpool")) && !docker.some((b) => b.t === "Stop" && !b.d), JSON.stringify(docker));
    ok("(Stop container on the Docker row)", await page.evaluate(() => {
      const li = [...document.querySelectorAll('li[role="row"]')].find((l) => l.innerText.includes("carpool-proxy-1"));
      const b = li && [...li.querySelectorAll("button")].find((x) => x.innerText.trim() === "Stop container");
      if (!b) return false;
      b.click();
      return true;
    }));
    await sleep(250);
    await page.evaluate(() => [...document.querySelectorAll(".fixed button")].find((b) => b.innerText.trim() === "Stop container")?.click());
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "ports_container_stop"), { timeout: 4000 }).catch(() => null);
    const cs = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "ports_container_stop")?.args ?? null);
    ok("  which stops that container by its id", cs?.id === "f50b929f94ba" && cs?.project === null, JSON.stringify(cs));
    await page.close();
  }
  {
    // A Stop the process ignored: Force stop appears, says what it does, and sends force.
    const page = await openPortsPage(browser, server, { killResult: { pid: 725, ended: false, how: "term", group: true, message: "node (pid 725) ignored the stop request for 3 s — Force stop sends SIGKILL." } });
    await page.waitForFunction(() => document.body.innerText.includes("of 5"), { timeout: 4000 }).catch(() => null);
    ok("(Stop on the node row)", await page.evaluate(() => {
      const li = [...document.querySelectorAll('li[role="row"]')].find((l) => l.innerText.includes("node · pid 725"));
      const b = li && [...li.querySelectorAll("button")].find((x) => x.innerText.trim() === "Stop");
      if (!b) return false;
      b.click();
      return true;
    }));
    await sleep(200);
    await page.evaluate(() => [...document.querySelectorAll(".fixed button")].find((b) => b.innerText.trim() === "Stop")?.click());
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Force stop"), { timeout: 4000 }).catch(() => null);
    ok("a process that ignored Stop gets a Force stop button", await page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Force stop")));
    await page.evaluate(() => [...document.querySelectorAll('li[role="row"] button')].find((b) => b.innerText.trim() === "Force stop")?.click());
    await sleep(200);
    const s = await text(page);
    ok("  whose confirmation says it ends the process at once", s.includes("SIGKILL"));
    await page.evaluate(() => [...document.querySelectorAll(".fixed button")].find((b) => b.innerText.trim() === "Force stop")?.click());
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "ports_kill").length >= 2, { timeout: 4000 }).catch(() => null);
    const forced = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "ports_kill").at(-1)?.args ?? null);
    ok("  and sends force", forced?.force === true && forced?.pid === 725, JSON.stringify(forced));
    await page.close();
  }
  {
    // Nothing from my folders: the empty state offers the rest of the machine.
    const page = await openPortsPage(browser, server, { rows: PORT_ROWS.filter((r) => !r.clone), storage: { "ports.onlyMine": true } });
    await page.waitForFunction(() => document.body.innerText.includes("Nothing is listening from your repositories' folders."), { timeout: 4000 }).catch(() => null);
    ok("with my folders holding nothing, the page says so and offers every port", (await text(page)).includes("Nothing is listening from your repositories' folders."));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.startsWith("Show every port"))?.click());
    await sleep(200);
    ok("  and that link shows them", (await text(page)).includes("mongod"));
    await page.close();
  }
  {
    // A slow listing never stacks: one request in flight, ticks skipped meanwhile.
    const page = await openPortsPage(browser, server, { latency: 7000 });
    await sleep(11000);
    const n = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "ports_list").length);
    ok("a listing that takes 7 s is asked at most twice in 11 s — never stacked", n <= 2, String(n));
    await page.close();
  }

  section("Two copies of one repository");
  {
    const page = await open(SCENARIOS.clean, { workspace: WORKSPACE_TWO });
    await page.waitForFunction(() => document.body.innerText.includes("copies of"), { timeout: 4000 }).catch(() => null);
    const s = await text(page);
    const at = s.indexOf("copies of");
    ok("the strip names both copies — this one first — each with its branch and changes", s.includes("2 copies of acme/argos among your repositories: Argos_06-10-26_V1 (this one, main, 3 changed) · Argos_08-10-26 (main, clean)"), s.slice(Math.max(0, at - 2), at + 140));
    ok("  no amber note while one copy is clean", !s.includes("Commits made in both will diverge"));
    const title = await page.evaluate(() => document.querySelector('[aria-label="Copies of this repository"]')?.getAttribute("title") ?? "");
    ok("  the row's title says what is left to the person", title.includes("never switches a branch, edits a port in your project, or renames containers"), title);
    const asked = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "workspace_info").map((c) => c.args.path));
    const before = (await asked()).length;
    ok("  it was read for the repository on screen", before >= 1 && (await asked()).every((p) => p === "/repos/gitswitch"), JSON.stringify(await asked()));
    await (await button(page, "Refresh")).click();
    await page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "workspace_info").length > n, { timeout: 4000 }, before).catch(() => null);
    ok("  Refresh reads the copies again", (await asked()).length > before);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty, { workspace: WORKSPACE_TWO_DIRTY });
    await page.waitForFunction(() => document.body.innerText.includes("Both copies have"), { timeout: 4000 }).catch(() => null);
    const s = await text(page);
    ok("the same branch with changes in both → the amber note, verbatim from the backend", s.includes("Both copies have main checked out with uncommitted changes (Argos_06-10-26_V1: 3 files, Argos_08-10-26: 1 file). Commits made in both will diverge and the second push will be refused — work on a branch per copy."));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    const s = await text(page);
    ok("a repository cloned once has no copies row", !s.includes("among your repositories"));
    await page.close();
  }
  {
    // Inside a submodule scope the copies are the superproject's business: the row is hidden.
    const page = await open(SCENARIOS.submodules, { submodules: SUBMODULES, subInner: SUB_INNER, workspace: WORKSPACE_TWO, storage: { "repo.sub": "trinity" } });
    await sleep(400);
    const s = await text(page);
    ok("inside a submodule scope the row is hidden", !s.includes("among your repositories"));
    await page.close();
  }
  {
    const page = await openReposPage(browser, server, { local: { clones: { "alice/a-1": "/w/a-1" }, owner_profiles: {}, org_ssh_users: {}, copies: { "alice/a-1": ["/w/a-1", "/w/other/a-1"] } } });
    await page.waitForFunction(() => document.body.innerText.includes("cloned ×2"), { timeout: 5000 }).catch(() => null);
    const s = await text(page);
    ok("the Repositories page marks a repository cloned twice", s.includes("cloned ×2"), s.slice(s.indexOf("a-1"), s.indexOf("a-1") + 80));
    const title = await page.evaluate(() => [...document.querySelectorAll("span[title]")].map((e) => e.title).find((t) => t.includes("/w/other/a-1")) ?? "");
    ok("  and names both folders", title.includes("/w/a-1") && title.includes("/w/other/a-1"), title);
    await page.close();
  }
  {
    const rows = [{ ...PORT_ROWS[0], copy: { rank: 2, of: 2, others: ["Argos_06-10-26_V1"] } }];
    const page = await openPortsPage(browser, server, { rows });
    await page.waitForFunction(() => document.body.innerText.includes("copy 2 of 2"), { timeout: 4000 }).catch(() => null);
    const s = await text(page);
    ok("a Ports row from a copy says which copy it is", s.includes("copy 2 of 2"));
    const title = await page.evaluate(() => [...document.querySelectorAll("span[title]")].map((e) => e.title).find((t) => t.startsWith("The other copy")) ?? "");
    ok("  and names the other one", title.includes("Argos_06-10-26_V1"), title);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("The workspace line");
  {
    const ports = { os: "macos", docker: "ok", took_ms: 1, notes: [], rows: [
      { ...PORT_ROWS[0], key: "tcp:3000:800", port: 3000, pid: 800, cwd: "/repos/gitswitch/apps/web", clone: { path: "/repos/gitswitch", name: "gitswitch", profile_name: "Kirmada" } },
      { ...PORT_ROWS[2], container: { ...PORT_ROWS[2].container, project: "argos_08-10-26-2f9c1a7b" } },
      PORT_ROWS[1],
    ] };
    const page = await open(SCENARIOS.clean, { workspace: WORKSPACE_TWO_OFFSET, ports });
    await page.waitForFunction(() => document.body.innerText.includes("Workspace #"), { timeout: 4000 }).catch(() => null);
    let s = await text(page);
    const at = s.indexOf("Workspace #");
    ok("the line names the workspace, its Compose project and its offset", s.includes("Workspace #2f9c1a7b") && s.includes("Compose project argos_08-10-26-2f9c1a7b") && s.includes("port offset 100"), s.slice(Math.max(0, at), at + 160));
    ok("  and counts the listeners from this checkout — a process in its folder and a container of its Compose project, not the rest", s.includes("2 listeners"), s.slice(Math.max(0, at), at + 160));
    const href = await page.evaluate(() => document.querySelector('a[title^="The Ports page"]')?.getAttribute("href") ?? "");
    ok("  (view) opens the Ports page filtered to this folder", href === "/ports?clone=%2Frepos%2Fgitswitch", href);
    const exportTitle = await page.evaluate(() => [...document.querySelectorAll("button[title]")].map((b) => b.title).find((t) => t.startsWith("export ")) ?? "");
    ok("  the export line carries the offset and the Compose project", exportTitle.startsWith("export GITSWITCH_PORT_OFFSET=100 COMPOSE_PROJECT_NAME=argos_08-10-26-2f9c1a7b"), exportTitle);
    // Changing the offset.
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "change")?.click());
    await page.waitForSelector('[aria-label="Port offset"]', { timeout: 3000 }).catch(() => null);
    await page.click('[aria-label="Port offset"]', { clickCount: 3 });
    await page.type('[aria-label="Port offset"]', "300");
    await (await button(page, "Save")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "workspace_set_offset"), { timeout: 4000 }).catch(() => null);
    const call = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "workspace_set_offset").at(-1)?.args ?? null);
    ok("Save stores the chosen offset for this repository", JSON.stringify(call) === JSON.stringify({ path: "/repos/gitswitch", offset: 300 }), JSON.stringify(call));
    await page.waitForFunction(() => document.body.innerText.includes("port offset 300"), { timeout: 4000 }).catch(() => null);
    s = await text(page);
    ok("  and the line shows it, with what does not change by itself", s.includes("port offset 300") && s.includes("Servers already running keep their ports"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { workspace: WORKSPACE_TWO_OFFSET, offsetRefusal: "Argos_06-10-26_V1 already uses port offset 0. Pick another, or change it there first." });
    await page.waitForFunction(() => document.body.innerText.includes("Workspace #"), { timeout: 4000 }).catch(() => null);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "change")?.click());
    await page.waitForSelector('[aria-label="Port offset"]', { timeout: 3000 }).catch(() => null);
    await page.click('[aria-label="Port offset"]', { clickCount: 3 });
    await page.type('[aria-label="Port offset"]', "0");
    await (await button(page, "Save")).click();
    await page.waitForFunction(() => document.body.innerText.includes("already uses port offset"), { timeout: 4000 }).catch(() => null);
    const s = await text(page);
    ok("an offset another copy uses is refused in the backend's words, without the error prefix", s.includes("Argos_06-10-26_V1 already uses port offset 0. Pick another, or change it there first.") && !s.includes("Configuration error:"));
    ok("  and the editor stays open for another try", (await page.$('[aria-label="Port offset"]')) !== null);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    await page.waitForFunction(() => document.body.innerText.includes("Workspace #"), { timeout: 4000 }).catch(() => null);
    const s = await text(page);
    ok("the first copy says it keeps Compose's own name and has no export line", s.includes("Compose project: Compose's own name") && !s.includes("copy export line"));
    ok("  with the listing unread, it says so rather than showing a zero", s.includes("listeners not read"));
    await page.close();
  }
  {
    const page = await openPortsPage(browser, server, { query: "?clone=%2FUsers%2Fme%2FDocuments%2FAcme%2Fgitswitch", storage: { "ports.onlyMine": false } });
    await page.waitForFunction(() => document.body.innerText.includes("1 of 5"), { timeout: 4000 }).catch(() => null);
    const value = await page.evaluate(() => document.querySelector('[aria-label="Filter ports"]')?.value ?? "");
    const s = await text(page);
    ok("the Ports page opened from the line filters by the folder", value === "/Users/me/Documents/Acme/gitswitch" && s.includes("1 of 5") && s.includes("node · pid 725"), `${value} / ${s.slice(Math.max(0, s.indexOf("of 5") - 4), s.indexOf("of 5") + 4)}`);
    ok("  and leaves the address clean", (await page.evaluate(() => location.search)) === "");
    await page.close();
  }

  // -----------------------------------------------------------------
  section("A gitswitch:// link that started the app");
  {
    const page = await browser.newPage();
    await page.setViewport({ width: 1280, height: 900 });
    await page.evaluateOnNewDocument(() => {
      window.__CALLS__ = [];
      localStorage.clear();
      window.__TAURI_INTERNALS__ = {
        transformCallback: () => 1,
        invoke: async (cmd, args) => {
          window.__CALLS__.push({ cmd, args });
          if (cmd === "plugin:event|listen") return 1;
          // The link came before the page listened: the backend kept it.
          if (cmd === "deep_link_take") return "/doctor";
          if (cmd === "get_profiles" || cmd.startsWith("doctor_check")) return [];
          return null;
        },
      };
    });
    await page.goto(`${server.url}/`, { waitUntil: "networkidle0" });
    await page.waitForFunction(() => location.pathname === "/doctor", { timeout: 4000 }).catch(() => null);
    const where = await page.evaluate(() => location.pathname);
    ok("a link that started the app opens the page it names", where === "/doctor", where);
    const order = await page.evaluate(() => window.__CALLS__.map((c) => c.cmd).filter((c) => c === "plugin:event|listen" || c === "deep_link_take"));
    ok("  collected once the page listens, so a link after that is not missed either", order.join(",") === "plugin:event|listen,deep_link_take", order.join(","));
    await page.close();
  }

  section("Clone page: Git LFS option");
  const CLONED_LFS = {
    path: "/repos/big-data",
    profile_name: "Kirmada",
    mapped_to: null,
    submodules: null,
    lfs: { installed: true, requested: true, tracked: 5, fetched: 5, pointers_left: 0, configured_now: true, error: null, note: "Downloaded 5 large files (Git LFS)." },
  };
  const CLONED_STUBS = {
    ...CLONED_LFS,
    lfs: { installed: true, requested: false, tracked: 5, fetched: 0, pointers_left: 5, configured_now: false, error: null, note: "5 large files are pointer stubs — open the repository in Changes → Git LFS → Pull LFS files to download them." },
  };
  {
    const page = await openClonePage(browser, server, {
      cloneHang: true,
      cloneProgress: { phase: "Receiving objects", percent: 45, done: 556, total: 1234, bytes: "10.00 MiB", rate: "2.00 MiB/s", estimate_kb: 1258291, eta_secs: 603 },
    });
    await page.waitForFunction(() => document.body.innerText.includes("About 1.2 GB on GitHub"), { timeout: 4000 });
    ok("before cloning, the page says how big GitHub says the repository is", true);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Clone").click());
    await page.waitForFunction(() => document.body.innerText.includes("Receiving objects 45%"), { timeout: 4000 });
    const s = await text(page);
    ok("while it runs, the banner shows git's phase and percentage", s.includes("Receiving objects 45%"));
    ok("  with how many objects, how much has arrived of the estimate, and the rate", s.includes("556 of 1,234") && s.includes("10.00 MiB of about 1.2 GB") && s.includes("2.00 MiB/s"));
    ok("  and how long is left at that rate", s.includes("about 10 min left"), s.slice(s.indexOf("2.00 MiB/s"), s.indexOf("2.00 MiB/s") + 40));
    ok("  as a bar", await page.evaluate(() => !!document.querySelector('[aria-label="Clone progress"]')));
    await page.close();
  }
  {
    const page = await openClonePage(browser, server, { url: "git@gh-work:me/big-data.git" });
    const shown = await page
      .waitForFunction(() => document.body.innerText.includes("About 1.2 GB on GitHub"), { timeout: 4000 })
      .then(() => true)
      .catch(() => false);
    const asked = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "clone_size_estimate").map((c) => c.args?.url));
    ok("an address through an SSH alias shows GitHub's size before cloning", shown && asked.includes("git@gh-work:me/big-data.git"), JSON.stringify(asked));
    const line = await page.evaluate(() => document.querySelector('[aria-label="Repository size"]')?.innerText ?? "");
    ok("  and says the submodules download on top of it", line.includes("submodules download on top"), line);
    await page.close();
  }
  {
    const page = await openClonePage(browser, server, { submodules: false });
    await page.waitForFunction(() => document.body.innerText.includes("About 1.2 GB on GitHub"), { timeout: 4000 }).catch(() => null);
    const line = await page.evaluate(() => document.querySelector('[aria-label="Repository size"]')?.innerText ?? "");
    ok("  but not when submodules are left out", line.includes("About 1.2 GB") && !line.includes("submodules"), line);
    await page.close();
  }
  const lfsBox = (page) => page.$('input[aria-label="Also download Git LFS files"]');
  const clickClone = (page) =>
    page.evaluate(() => {
      const b = [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Clone");
      if (!b) return false;
      b.click();
      return true;
    });
  {
    const page = await openClonePage(browser, server, { cloneResult: CLONED_LFS });
    const box = await lfsBox(page);
    ok("the LFS checkbox is offered", box !== null);
    ok("  and on by default", await box.evaluate((el) => el.checked));
    const s = await text(page);
    ok("  explaining what happens without it", s.includes("pointer stubs"));
    ok("  and that it can be done later from Changes", s.includes("Changes → Git LFS"));
    ok("the Clone button is there", await clickClone(page));
    await sleep(400);
    const s2 = await text(page);
    ok("the result says what arrived", s2.includes("Downloaded 5 large files"));
    ok("  and the clone was asked to fetch LFS", await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "full_clone" && c.args.withLfs === true)));
    await page.close();
  }
  {
    // Refresh does what leaving and re-opening the page does: the last clone's result card goes; the form stays.
    const page = await openClonePage(browser, server, { cloneResult: CLONED_LFS });
    await clickClone(page);
    await page.waitForFunction(() => document.body.innerText.includes("Cloned big-data"), { timeout: 4000 });
    const refresh = await button(page, "Refresh");
    ok("after the clone the result card is shown, and the page has Refresh", refresh !== null);
    await refresh.click();
    await page.waitForFunction(() => !document.body.innerText.includes("Cloned big-data"), { timeout: 4000 });
    const s = await text(page);
    ok("Refresh drops the result card, as leaving the page would", !s.includes("Cloned big-data") && !s.includes("Downloaded 5 large files"));
    ok("  and keeps the form", await page.evaluate(() => [...document.querySelectorAll("input")].some((i) => i.value.includes("big-data"))));
    await page.close();
  }
  {
    // A failed clone's red message goes with Refresh too.
    const page = await openClonePage(browser, server, { cloneError: "git clone failed: Repository not found." });
    await clickClone(page);
    await page.waitForFunction(() => document.body.innerText.includes("Repository not found"), { timeout: 4000 }).catch(() => null);
    ok("a failed clone shows its error", (await text(page)).includes("Repository not found"));
    await (await button(page, "Refresh"))?.click();
    await page.waitForFunction(() => !document.body.innerText.includes("Repository not found"), { timeout: 4000 }).catch(() => null);
    ok("  and Refresh drops it, as leaving the page would", !(await text(page)).includes("Repository not found"));
    await page.close();
  }
  {
    const page = await openClonePage(browser, server, { cloneResult: CLONED_STUBS });
    const box = await lfsBox(page);
    await box.evaluate((el) => el.click());
    await sleep(100);
    ok("the option can be turned off", !(await lfsBox(page).then((b) => b.evaluate((el) => el.checked))));
    await clickClone(page);
    await sleep(400);
    const s = await text(page);
    ok("a clone without it says the stubs remain", s.includes("5 large files are pointer stubs"));
    ok("  and points at the Changes page", s.includes("Changes → Git LFS"));
    ok("  with a button to go there", (await button(page, "Open in Changes")) !== null);
    ok("  the clone was asked NOT to fetch LFS", await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "full_clone" && c.args.withLfs === false)));
    await page.close();
  }
  {
    const page = await openClonePage(browser, server, { lfsTool: { installed: false, version: null, install_hint: "brew install git-lfs" }, cloneResult: CLONED_STUBS });
    const box = await lfsBox(page);
    ok("without git-lfs the checkbox is disabled", await box.evaluate((el) => el.disabled));
    const s = await text(page);
    ok("  and says how to install it", s.includes("git-lfs isn't installed") && s.includes("brew install git-lfs"));
    await page.close();
  }
  {
    const page = await openClonePage(browser, server, { mode: "sparse", cloneResult: CLONED_STUBS });
    const box = await lfsBox(page);
    ok("a sparse clone offers the option too", box !== null);
    ok("  saying it happens when folders are checked out", (await text(page)).includes("when you check folders out"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Branches");
  const openBranches = async (page) => {
    await (await button(page, "Branches")).click();
    await page.waitForFunction(() => document.body.innerText.includes("feature/login"), { timeout: 4000 });
  };
  const rowWith = (page, needle) =>
    page.evaluate((n) => {
      const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes(n));
      if (!row) return null;
      return {
        text: row.innerText,
        buttons: [...row.querySelectorAll("button")].map((b) => ({ text: b.innerText.trim(), title: b.title, disabled: b.disabled })),
      };
    }, needle);
  {
    const page = await open(SCENARIOS.clean);
    await openBranches(page);
    const s = await text(page);
    ok("Branches opens a dialog listing the local branches", s.includes("Branches") && s.includes("feature/login") && s.includes("old-experiment"));
    ok("  saying how far ahead each one is", s.includes("↑3"));
    ok("  remote-tracking refs are not rows", !(await rowWith(page, "origin/main"))?.text.startsWith("origin/main"));
    const head = await rowWith(page, "HEAD");
    ok("  the current branch carries a HEAD badge", head !== null && head.text.includes("main"));
    ok("  and has no Switch button", head !== null && !head.buttons.some((b) => b.title === "Switch to this branch"));
    const del = head?.buttons.find((b) => b.text === "Delete…");
    ok("  its Delete… is disabled and says why", del?.disabled === true && del.title.includes("branch you're on"));
    ok("Switch is offered on the other rows", await clickInRow(page, "feature/login", { title: "Switch to this branch" }));
    await sleep(300);
    const sw = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_switch_branch"));
    ok("  and sends the branch name", sw?.args?.repoPath === "/repos/gitswitch" && sw?.args?.name === "feature/login");
    ok("  success closes the dialog", !(await text(page)).includes("Create branch"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    await openBranches(page);
    ok("Create branch waits for a name", (await buttonState(page, "Create branch"))?.disabled === true);
    await page.type('input[aria-label="New branch name"]', "feature/signup");
    ok("  the switch checkbox is on by default", await page.$eval('input[aria-label="Switch to the new branch"]', (el) => el.checked));
    await (await button(page, "Create branch")).click();
    await sleep(300);
    const cr = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_create_branch"));
    ok("  and sends the name, the start point and the switch choice", cr?.args?.name === "feature/signup" && cr?.args?.from === "main" && cr?.args?.switchTo === true);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, {
      refuse: { changes_delete_branch: { code: "unmerged-branch", message: "Deleting `feature/login` would drop 3 commit(s) that no other branch has." } },
    });
    await openBranches(page);
    ok("Delete… is offered", await clickInRow(page, "feature/login", { title: "Delete this branch" }));
    await page.waitForFunction(() => document.body.innerText.includes("Delete feature/login?"), { timeout: 4000 });
    const s = await text(page);
    ok("an unmerged branch asks again, naming the commits it would drop", s.includes("Delete feature/login?") && s.includes("3 commit"));
    ok("  and promises the undo command", s.includes("undo command is shown afterwards"));
    // The refusal is the question, not a failure: no red toast, no failed-operation card behind the dialog.
    ok("  asked once — no error toast and no result card for the question", s.split("would drop 3 commit(s)").length === 2 && (await button(page, "dismiss")) === null);
    const first = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_delete_branch"));
    ok("  the first attempt was not forced", first.length === 1 && first[0].args.force === false);
    await (await button(page, "Delete anyway")).click();
    await sleep(300);
    const calls = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_delete_branch"));
    ok("  Delete anyway re-sends with force", calls.length === 2 && calls[1].args.force === true && calls[1].args.name === "feature/login");
    ok("  and the result carries the undo command", (await text(page)).includes("undo: git -C /repos/gitswitch branch feature/login"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty, {
      refuse: { changes_switch_branch: { code: "dirty-tree", message: "Switching would overwrite your uncommitted changes.", guidance: "Commit or stash these files first: src/app.ts" } },
    });
    await openBranches(page);
    await clickInRow(page, "feature/login", { title: "Switch to this branch" });
    await page.waitForFunction(() => document.body.innerText.includes("src/app.ts"), { timeout: 4000 });
    const s = await text(page);
    ok("a refused switch keeps the dialog open", s.includes("Create branch"));
    ok("  with git's reason and the blocking files", s.includes("overwrite your uncommitted changes") && s.includes("src/app.ts"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.detached);
    const s = await text(page);
    ok("a detached HEAD is said in words, with the commit", s.includes("detached HEAD at 1a2b3c4") && s.includes("not on a branch"));
    ok("  and publishing is disabled (nothing to push from)", (await buttonState(page, "Publish branch"))?.disabled === true);
    const offers = await page.evaluate(() => [...document.querySelectorAll("button")].filter((b) => b.innerText.trim() === "Choose a branch").length);
    ok("  the commit box offers to choose a branch", offers === 1);
    ok("  and Preview update says why it waits", await (await button(page, "Preview update")).evaluate((b) => b.disabled && b.title.includes("Check out a branch first")));
    await (await button(page, "Choose a branch")).click();
    await page.waitForFunction(() => document.body.innerText.includes("this commit (detached HEAD)"), { timeout: 4000 });
    ok("  which opens the dialog starting from this commit", true);
    await page.type('input[aria-label="New branch name"]', "rescue");
    await (await button(page, "Create branch")).click();
    await sleep(300);
    const cr = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_create_branch"));
    ok("  creating from there sends no start point (HEAD)", cr?.args?.name === "rescue" && cr?.args?.from === null);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    await openBranches(page);
    await clickInRow(page, "feature/login", { title: "Rename this branch" });
    await page.waitForSelector('input[aria-label="Rename to"]', { timeout: 4000 });
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Rename to"]');
      i.focus();
      i.select();
    });
    await page.keyboard.type("feature/signin");
    ok("Rename… turns the row into a field with Save", await clickInRow(page, "feature/login", { text: "Save" }));
    await sleep(300);
    const rn = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_rename_branch"));
    ok("  and Save sends the old and new names", rn?.args?.oldName === "feature/login" && rn?.args?.newName === "feature/signin");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.rebasing);
    const b = await buttonState(page, "Branches");
    ok("during a rebase Branches is disabled", b?.disabled === true);
    ok("  saying to finish or abort first", (b?.title ?? "").includes("Finish or abort the rebase first"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Stashes");
  {
    const page = await open(SCENARIOS.withStashes, { stashes: STASHES });
    await page.waitForFunction(() => document.body.innerText.includes("fix header"), { timeout: 4000 });
    const s = await text(page);
    ok("the stash list is a card with its count", s.includes("Stashes 2") && s.includes("fix header"));
    ok("  each entry names its ref, branch and date", s.includes("stash@{0}") && s.includes("on main") && s.includes("2026-09-23"));
    ok("  the passive note is gone", !s.includes("stashes in this repository"));
    await (await button(page, "Stash changes…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Stash changes"), { timeout: 4000 });
    const s1 = await text(page);
    ok("Stash changes… explains what it sets aside, with counts", s1.includes("5 uncommitted changes") && s1.includes("(2 staged, 2 not staged, 1 new)"));
    ok("  the untracked choice is offered and on", await page.$eval('input[aria-label="Include untracked files"]', (el) => el.checked));
    await page.type('input[aria-label="Stash message"]', "wip");
    await (await button(page, "Stash")).click();
    await sleep(300);
    const push = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_stash_push"));
    ok("  Stash sends the message and the untracked choice", push?.args?.message === "wip" && push?.args?.includeUntracked === true);
    ok("Apply is offered", await clickInRow(page, "fix header", { title: "Apply this stash and keep it in the list" }));
    await sleep(300);
    const ap = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_stash_apply"));
    ok("  and applies without popping", ap?.args?.index === 0 && ap?.args?.pop === false);
    ok("  naming the row's commit, so a shifted list is refused rather than acted on", ap?.args?.oid === STASHES[0].oid);
    await clickInRow(page, "fix header", { title: "Apply this stash and remove it" });
    await sleep(300);
    const pop = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stash_apply")[1]);
    ok("Pop applies and removes", pop?.args?.index === 0 && pop?.args?.pop === true && pop?.args?.oid === STASHES[0].oid);
    await page.close();
  }
  {
    // The status carries a fresh stash count on every read (Refresh, focus); the list follows it.
    const page = await open(SCENARIOS.withStashes, { stashes: STASHES });
    await page.waitForFunction(() => document.body.innerText.includes("fix header"), { timeout: 4000 });
    const lists = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stash_list").length);
    const before = await lists();
    await page.evaluate(() => {
      window.__SCENARIO__ = { ...window.__SCENARIO__, stash_count: 3 };
    });
    await (await button(page, "Refresh")).click();
    await sleep(400);
    ok("a Refresh that finds a different stash count re-reads the list", (await lists()) > before);
    ok("  and the header trusts the status's count over the rows it has", (await text(page)).includes("Stashes 3"));
    await page.close();
  }
  {
    const page = await open({ ...SCENARIOS.withStashes, stash_count: 60 }, { stashes: STASHES_CAPPED });
    await page.waitForFunction(() => document.body.innerText.includes("wip 49"), { timeout: 4000 });
    const s = await text(page);
    ok("with more stashes than the backend lists, the header shows the real total", s.includes("Stashes 60"));
    ok("  and says the list is cut", s.includes("Showing the first 50 of 60 stashes"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.withStashes, {
      stashes: STASHES,
      refuse: { changes_stash_drop: { code: "no-stash", message: "stash@{0} is no longer that entry — the list may be out of date." } },
    });
    await page.waitForFunction(() => document.body.innerText.includes("fix header"), { timeout: 4000 });
    const lists = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stash_list").length);
    const before = await lists();
    await clickInRow(page, "fix header", { title: "Drop this stash" });
    await page.waitForFunction(() => document.body.innerText.includes("Drop this stash?"), { timeout: 4000 });
    await (await button(page, "Drop")).click();
    await page.waitForFunction(() => document.body.innerText.includes("out of date"), { timeout: 4000 });
    const dr = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_stash_drop"));
    ok("Drop names the row's commit", dr?.args?.index === 0 && dr?.args?.oid === STASHES[0].oid);
    ok("  a no-stash refusal says the list was stale", (await text(page)).includes("The list was out of date"));
    await sleep(200);
    ok("  and the list was re-read", (await lists()) > before);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.withStashes, { stashes: STASHES });
    await page.waitForFunction(() => document.body.innerText.includes("fix header"), { timeout: 4000 });
    ok("Drop… is offered", await clickInRow(page, "fix header", { title: "Drop this stash" }));
    await page.waitForFunction(() => document.body.innerText.includes("Drop this stash?"), { timeout: 4000 });
    const s = await text(page);
    ok("dropping asks first, naming the entry", s.includes("Drop this stash?") && s.includes("stash@{0}") && s.includes("4 files"));
    ok("  and says the commit can be recovered", s.includes("can be recovered"));
    ok("  nothing was dropped yet", (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stash_drop").length)) === 0);
    await (await button(page, "Drop")).click();
    await page.waitForFunction(() => document.body.innerText.includes("undo: git -C"), { timeout: 4000 });
    ok("  after Drop the undo command is shown", (await text(page)).includes("stash store -m 'fix header'"));
    ok("the files can be listed", await clickInRow(page, "fix header", { text: "4 files" }));
    await page.waitForFunction(() => document.body.innerText.includes("Restore file"), { timeout: 4000 });
    const s2 = await text(page);
    ok("  with each path and an untracked marker", s2.includes("src/header.ts") && s2.includes("notes.txt"));
    const shows = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stash_show").length);
    const shown = await shows();
    await clickInRow(page, "fix header", { title: "Restore only this file from the stash" });
    await sleep(400);
    const rf = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_stash_restore_file"));
    ok("  Restore file sends the index and that path only", rf?.args?.index === 0 && rf?.args?.path === "src/header.ts");
    ok("  with the row's commit", rf?.args?.oid === STASHES[0].oid);
    // The operation re-reads the list; an open file list must be re-read with it, not left spinning.
    const s3 = await text(page);
    ok("  afterwards the open file list is re-read, not left at 'Reading the stash…'", (await shows()) === shown + 1 && s3.includes("src/header.ts") && !s3.includes("Reading the stash…"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    const b = await buttonState(page, "Stash changes…");
    ok("on a clean tree Stash changes… is disabled", b?.disabled === true);
    ok("  saying there is nothing to stash", b?.title === "Nothing to stash");
    ok("  and no Stashes card appears", !(await text(page)).includes("Stashes"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Tidy up");
  {
    const page = await open(SCENARIOS.dirty);
    ok("Undo last commit is offered", (await buttonState(page, "Undo last commit"))?.disabled === false);
    await (await button(page, "Undo last commit")).click();
    await sleep(300);
    const undo = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_undo_commit"));
    ok("  and runs without a confirmation", undo?.args?.repoPath === "/repos/gitswitch");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.pushedAlready);
    const b = await buttonState(page, "Undo last commit");
    ok("a pushed commit can't be undone here", b?.disabled === true);
    // can_amend is false when ANY remote branch holds HEAD, so the tooltip must not name the upstream.
    ok("  and the tooltip points at History without naming the upstream", (b?.title ?? "").includes("History") && (b?.title ?? "").includes("a remote branch") && !(b?.title ?? "").includes("origin/main"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { refuse: { changes_undo_commit: { code: "already-pushed", message: "The last commit is already on a remote branch. Revert it instead." } } });
    await (await button(page, "Undo last commit")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Open in History"), { timeout: 4000 });
    ok("a refusal because it was pushed offers to open History", (await text(page)).includes("already on a remote branch"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged);
    const b = await buttonState(page, "Reset to origin/main…");
    ok("Reset names the upstream on the button", b !== null && b.disabled === false);
    await (await button(page, "Reset to origin/main…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Reset to origin/main?"), { timeout: 4000 });
    const s = await text(page);
    ok("resetting asks first, counting the unpushed commits", s.includes("Reset to origin/main?") && s.includes("2 unpushed"));
    ok("  and promises a backup branch", s.includes("gitswitch-before-reset-"));
    ok("  a clean tree offers no stash choice", (await page.$('input[aria-label="Stash uncommitted changes first"]')) === null);
    ok("  nothing was reset yet", (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_reset").length)) === 0);
    await (await button(page, "Reset")).click();
    await page.waitForFunction(() => document.body.innerText.includes("undo: git -C"), { timeout: 4000 });
    const rs = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_reset"));
    ok("  Reset is a hard reset to @{u} without stashing", rs?.args?.target === "@{u}" && rs?.args?.mode === "hard" && rs?.args?.stashFirst === false);
    const s2 = await text(page);
    ok("  the result names the backup branch and the undo command", s2.includes("gitswitch-before-reset-20260923120000") && s2.includes("undo: git -C /repos/gitswitch reset --hard"));
    ok("  and which commits left the branch", s2.includes("seal work"));
    await page.close();
  }
  {
    // A hard go-back that leaves a submodule ahead of the record offers to put it back.
    const page = await open(SCENARIOS.diverged, { treeReset: { ...TREE_RESET_OUTCOME, submodule_mismatch: ["trinity"] } });
    await (await button(page, "Reset to origin/main…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Reset to origin/main?"), { timeout: 4000 });
    await (await button(page, "Reset")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Align submodules checks out"), { timeout: 4000 }).catch(() => null);
    ok("a go-back that leaves a submodule elsewhere names it and says what Align submodules does", (await text(page)).includes("trinity sits at a commit other than what this repository records. Align submodules checks out the commit it records"));
    ok("  and offers the button", (await buttonState(page, "Align submodules"))?.disabled === false);
    await (await button(page, "Align submodules")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_submodule_align"), { timeout: 4000 }).catch(() => null);
    const al = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_submodule_align").at(-1)?.args ?? null);
    ok("  which checks out the named submodules, in the repository", al?.repoPath === "/repos/gitswitch" && JSON.stringify(al?.paths) === '["trinity"]', JSON.stringify(al));
    await page.close();
  }
  {
    const page = await open({ ...SCENARIOS.diverged, entries: SCENARIOS.dirty.entries, staged_count: 2, unstaged_count: 3, untracked_count: 1 });
    await (await button(page, "Reset to origin/main…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Reset to origin/main?"), { timeout: 4000 });
    const box = await page.$('input[aria-label="Stash uncommitted changes first"]');
    ok("a dirty tree offers to stash first", box !== null);
    ok("  checked by default", await box.evaluate((el) => el.checked));
    ok("  counting the changes", (await text(page)).includes("Stash my 5 uncommitted changes first"));
    await box.evaluate((el) => el.click());
    await sleep(100);
    ok("  unchecking warns that they are thrown away", (await text(page)).includes("Without stashing, your 5 uncommitted changes are thrown away"));
    await box.evaluate((el) => el.click());
    await sleep(100);
    await (await button(page, "Reset")).click();
    await sleep(300);
    const rs = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_reset"));
    ok("  Reset sends stashFirst", rs?.args?.target === "@{u}" && rs?.args?.mode === "hard" && rs?.args?.stashFirst === true);
    await page.close();
  }
  {
    // Unticking the box is the consent the backend honours: the changes are thrown away.
    const page = await open({ ...SCENARIOS.diverged, entries: SCENARIOS.dirty.entries, staged_count: 2, unstaged_count: 3, untracked_count: 1 });
    await (await button(page, "Reset to origin/main…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Reset to origin/main?"), { timeout: 4000 });
    await page.$eval('input[aria-label="Stash uncommitted changes first"]', (el) => el.click());
    await sleep(100);
    ok("with the stash box unticked Reset stays enabled (the red warning is the consent)", (await buttonState(page, "Reset"))?.disabled === false);
    await (await button(page, "Reset")).click();
    await sleep(300);
    const rs = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_reset"));
    ok("  and sends stashFirst false", rs?.args?.mode === "hard" && rs?.args?.stashFirst === false);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    const b = await buttonState(page, "Reset to origin/main…");
    ok("level with upstream: Reset is disabled", b?.disabled === true);
    ok("  saying so", (b?.title ?? "").includes("Already level with origin/main"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.unpublished);
    const b = await buttonState(page, "Reset to upstream…");
    ok("without an upstream the button says upstream and is disabled", b !== null && b.disabled === true);
    ok("  with the reason", b?.title === "This branch has no upstream");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty);
    await (await button(page, "Discard everything…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Discard everything?"), { timeout: 4000 });
    const s = await text(page);
    // Four distinct files carry the 2 staged + 3 unstaged changes; the new file is not among them until ticked.
    ok("Discard everything asks first, counting only what it touches", s.includes("changes in 4 files (2 staged, 3 not staged)") && s.includes("cannot be undone"));
    ok("  new files are kept unless asked", await page.$eval('input[aria-label="Also delete new files"]', (el) => !el.checked));
    ok("  a stash is offered instead", (await page.$('input[aria-label="Stash them first"]')) !== null);
    await page.$eval('input[aria-label="Also delete new files"]', (el) => el.click());
    await sleep(100);
    ok("  ticking it adds the new file to what is thrown away", (await text(page)).includes("changes in 5 files (2 staged, 3 not staged) and deletes 1 new file"));
    await page.$eval('input[aria-label="Also delete new files"]', (el) => el.click());
    await sleep(100);
    await (await button(page, "Discard everything")).click();
    await sleep(300);
    const da = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_discard_all"));
    ok("  and sends the defaults", da?.args?.includeUntracked === false && da?.args?.stashFirst === false);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean);
    ok("on a clean tree Discard everything… is disabled", (await buttonState(page, "Discard everything…"))?.disabled === true);
    await page.close();
  }
  {
    // Only new files: the backend refuses "nothing to discard" unless they are to be deleted.
    const page = await open(SCENARIOS.untrackedOnly);
    ok("with only new files Discard everything… is offered", (await buttonState(page, "Discard everything…"))?.disabled === false);
    await (await button(page, "Discard everything…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Discard everything?"), { timeout: 4000 });
    const b = await buttonState(page, "Discard everything");
    ok("  but confirming waits for the new files to be ticked, saying so", b?.disabled === true && b.title.includes("tick the box"));
    ok("  and the dialog says only new files are here", (await text(page)).includes("only 1 new file"));
    await page.$eval('input[aria-label="Also delete new files"]', (el) => el.click());
    await sleep(100);
    ok("  ticking it enables Discard everything", (await buttonState(page, "Discard everything"))?.disabled === false);
    await (await button(page, "Discard everything")).click();
    await sleep(300);
    const da = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_discard_all"));
    ok("  which sends includeUntracked", da?.args?.includeUntracked === true);
    await page.close();
  }
  {
    // Conflicts left by a stash pop, nothing else: the backend allows discarding them in one go.
    const page = await open(SCENARIOS.stashConflict);
    ok("leftover stash conflicts alone keep Discard everything… enabled", (await buttonState(page, "Discard everything…"))?.disabled === false);
    await (await button(page, "Discard everything…")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Discard everything?"), { timeout: 4000 });
    ok("  and the dialog counts them", (await text(page)).includes("changes in 1 file (0 staged, 0 not staged, 1 conflicted)"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty);
    await (await button(page, "Delete all untracked")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Delete these new files?"), { timeout: 4000 });
    const s = await text(page);
    ok("Delete all untracked asks first", s.includes("This deletes 1 untracked file from disk") && s.includes("cannot be undone"));
    ok("  and says ignored files are safe", s.includes("Ignored files are never touched"));
    ok("  listing the file", s.includes("new file.txt"));
    await (await button(page, "Delete")).click();
    await sleep(300);
    const d = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_discard"));
    ok("  Delete sends only the untracked paths", Array.isArray(d?.args?.paths) && d.args.paths.length === 1 && d.args.paths[0] === "new file.txt");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty, { pullMode: "rebase" });
    ok("rebase on a dirty tree: Pull is disabled", (await buttonState(page, "Pull"))?.disabled === true);
    const box = await page.$('input[aria-label="Stash changes around the rebase"]');
    ok("  and autostash is offered, off", box !== null && !(await box.evaluate((el) => el.checked)));
    ok("  the refusal mentions it", (await text(page)).includes("turn on autostash below"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty, { pullMode: "rebase", autostash: true });
    ok("with autostash on, Pull is enabled", (await buttonState(page, "Pull"))?.disabled === false);
    await (await button(page, "Pull")).click();
    await sleep(300);
    const pull = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_pull"));
    ok("  and the pull asks for autostash", pull?.args?.mode === "rebase" && pull?.args?.autostash === true);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.dirty, { pullMode: "merge", autostash: true });
    ok("autostash only shows for rebase", (await page.$('input[aria-label="Stash changes around the rebase"]')) === null);
    await (await button(page, "Pull")).click();
    await sleep(300);
    const pull = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_pull"));
    ok("  and is not sent for a merge", pull?.args?.autostash === false);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Submodule sections");
  const header = (page, path) =>
    page.evaluate((p) => {
      const b = document.querySelector(`button[aria-label="Toggle submodule ${p}"]`);
      return b ? { text: b.innerText, expanded: b.getAttribute("aria-expanded") } : null;
    }, path);
  const subOpts = { submodules: [...SUBMODULES, SUBMODULE_A], subInner: SUB_INNER, submoduleStatuses: SUB_STATUSES };
  {
    const page = await open(SCENARIOS.subSections, subOpts);
    await page.waitForFunction(() => document.body.innerText.includes("Inside B"), { timeout: 4000 });
    const s = await text(page);
    ok("each submodule with work gets its own section", s.includes("Inside A") && s.includes("Inside B"));
    const a = await header(page, "A");
    const b = await header(page, "B");
    ok("A's header says where it is and how far ahead", a !== null && a.text.includes("on main") && a.text.includes("↑1"));
    ok("  and how much changed", a.text.includes("2 changed"));
    ok("B's header flags the detached HEAD and the rebase", b !== null && b.text.includes("detached") && b.text.includes("rebase in progress"));
    ok("  and its conflict", b.text.includes("1 conflict"));
    ok("B is open by default (it has a conflict)", b.expanded === "true");
    ok("A is open too (two sections or fewer)", a.expanded === "true");
    ok("A's files are listed", s.includes("inside/a.ts"));
    ok("  with a Commit in A button", (await button(page, "Commit in A")) !== null);
    ok("  while the parent's own Commit button is still there", (await buttonState(page, "Commit"))?.text === "Commit");
    ok("B's rebase is shown inside its section", s.includes("Rebase in progress inside B") && (await button(page, "Abort in B")) !== null);
    ok("  with Continue disabled while the conflict remains", (await buttonState(page, "Continue in B"))?.disabled === true);
    ok("  and its conflict names the sides", s.includes("mine = your commit being replayed") && s.includes("theirs = origin/main"));
    // A gitlink row can't be discarded from the parent (the backend refuses the whole request when one is selected).
    const gitlink = await rowWith(page, "points at a different commit");
    ok("the parent's gitlink rows offer no Discard", gitlink !== null && !gitlink.buttons.some((b) => b.title === "Discard — this cannot be undone"));
    const discardAlls = await page.evaluate(() => ({
      total: [...document.querySelectorAll("button")].filter((b) => b.innerText.trim() === "Discard all").length,
      inA: [...document.querySelectorAll("#submodule-section-A button")].filter((b) => b.innerText.trim() === "Discard all").length,
    }));
    ok("  and with only gitlinks unstaged the parent has no Discard all (A's own list still does)", discardAlls.total === 1 && discardAlls.inA === 1, JSON.stringify(discardAlls));
    ok("staging a file inside A", await clickInRow(page, "long/name.ts", { title: "Stage" }));
    await sleep(300);
    const st = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_stage"));
    ok("  runs in A's repository", st?.args?.repoPath === "/repos/gitswitch/A" && st.args.paths[0].endsWith("long/name.ts"));
    ok("unstaging a file inside A", await clickInRow(page, "inside/a.ts", { title: "Unstage" }));
    await sleep(300);
    const un = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_unstage"));
    ok("  runs in A's repository too", un?.args?.repoPath === "/repos/gitswitch/A" && un.args.paths[0] === "inside/a.ts");
    ok("  and the parent was re-read afterwards", (await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status" && c.args.repoPath === "/repos/gitswitch").length)) >= 2);
    await page.close();
  }
  {
    const refusal = { code: "conflict-markers", message: "long/name.ts still contains conflict markers (line 3). Finish the edit, or Keep mine / Take theirs." };
    const page = await open(SCENARIOS.subSections, { ...subOpts, stageRefusal: refusal });
    await page.waitForFunction(() => document.body.innerText.includes("Inside A"), { timeout: 4000 });
    const stages = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stage").map((c) => c.args));
    const anyway = () => page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Mark resolved anyway"), { timeout: 4000 });
    ok("staging inside A is refused for markers", await clickInRow(page, "long/name.ts", { title: "Stage" }));
    await anyway();
    await (await button(page, "Mark resolved anyway")).click();
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "changes_stage").length >= 2, { timeout: 4000 });
    const calls = await stages();
    ok(
      "  and the retry runs inside A on the same file, markers allowed — not on the parent",
      calls.length === 2 && calls.every((c) => c.repoPath === "/repos/gitswitch/A") && calls[1].allowMarkers === true && calls[1].paths[0] === calls[0].paths[0],
      JSON.stringify(calls)
    );
    await sleep(200);
    ok("  the retry's own result takes the offer away", (await button(page, "Mark resolved anyway")) === null);
    // Refused again; then something else happens.
    await clickInRow(page, "long/name.ts", { title: "Stage" });
    await anyway();
    await clickInRow(page, "inside/a.ts", { title: "Unstage" });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_unstage"), { timeout: 4000 });
    await sleep(300);
    ok("  and so does the result of any other operation", (await button(page, "Mark resolved anyway")) === null);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.subSections, subOpts);
    await page.waitForFunction(() => document.body.innerText.includes("Inside A"), { timeout: 4000 });
    await page.type("#submodule-section-A textarea", "Fix the header inside A");
    await (await button(page, "Commit in A")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Stage pointer"), { timeout: 4000 });
    const cm = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_commit"));
    ok("Commit in A commits inside A with its draft", cm?.args?.repoPath === "/repos/gitswitch/A" && cm.args.message === "Fix the header inside A");
    const s = await text(page);
    ok("  then the footer says the parent still records the old commit", s.includes("Committed 9f1e2d3 inside A") && s.includes("still records the old commit for A"));
    ok("  offering both ways to record it", (await button(page, "Stage pointer")) !== null && (await button(page, "Record submodule pointers")) !== null);
    await (await button(page, "Stage pointer")).click();
    await sleep(300);
    const sp = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stage").pop());
    ok("  Stage pointer stages the gitlink in the parent", sp?.args?.repoPath === "/repos/gitswitch" && sp.args.paths.length === 1 && sp.args.paths[0] === "A");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.subSections, subOpts);
    await page.waitForFunction(() => document.body.innerText.includes("Inside A"), { timeout: 4000 });
    const row = await rowWith(page, "points at a different commit");
    ok("the parent's gitlink row for A has an Inside link", row !== null && row.buttons.some((b) => b.title === "Show the changes inside this submodule"));
    await page.evaluate(() => document.querySelector('button[aria-label="Toggle submodule A"]').click());
    await sleep(100);
    ok("  the section can be collapsed", (await header(page, "A"))?.expanded === "false");
    await clickInRow(page, "points at a different commit", { title: "Show the changes inside this submodule" });
    await sleep(200);
    ok("  and Inside opens it again", (await header(page, "A"))?.expanded === "true");
    await page.waitForFunction(() => document.body.innerText.includes("moved 1 commit ahead"), { timeout: 4000 });
    const before = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status" && c.args.repoPath === "/repos/gitswitch/A").length);
    await clickInRow(page, "moved 1 commit ahead", { text: "Show what changed" });
    await sleep(300);
    const s = await text(page);
    ok("the side card's row for A links to the section instead of re-reading it", s.includes("See the changes inside") && /commits it moved through/i.test(s));
    const after = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status" && c.args.repoPath === "/repos/gitswitch/A").length);
    ok("  without a second status read", after === before);
    await clickInRow(page, "staging", { text: "Show what changed" });
    await page.waitForFunction(() => document.body.innerText.includes("Changed inside"), { timeout: 4000 });
    const readOnly = await page.evaluate(() => {
      const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes("inside/changed.ts"));
      if (!row) return "row missing";
      const titles = [...row.querySelectorAll("button")].map((b) => b.title);
      return titles.some((x) => /Stage|Discard|Unstage/.test(x)) ? "has actions" : "read-only";
    });
    ok("a submodule without a section keeps the read-only inner list", readOnly === "read-only", readOnly);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.subSections, subOpts);
    await page.waitForFunction(() => document.body.innerText.includes("Inside A"), { timeout: 4000 });
    await (await button(page, "Open as its own repository")).click();
    await sleep(500);
    const picked = await page.evaluate(() => JSON.parse(localStorage.getItem("gitswitch:changes.repo") ?? '""'));
    ok("Open as its own repository switches the page to A", picked === "/repos/gitswitch/A", picked);
    ok("  reading A as the main repository", await page.evaluate(() => window.__CALLS__.some((c) => c.cmd === "changes_repo_status" && c.args.repoPath === "/repos/gitswitch/A")));
    ok("  which has no sections of its own", !(await text(page)).includes("Inside A"));
    await page.close();
  }
  {
    const resolvedB = { ...SUB_STATUSES[1], status: { ...SUB_STATUSES[1].status, entries: [], conflicted_count: 0 } };
    const page = await open(SCENARIOS.subSections, { ...subOpts, submoduleStatuses: [SUB_STATUSES[0], resolvedB, SUB_QUIET] });
    await page.waitForFunction(() => document.body.innerText.includes("Inside B"), { timeout: 4000 });
    ok("a submodule with nothing to do is one line, not a section", (await text(page)).includes("1 other submodule has nothing to commit") && !(await text(page)).includes("Inside C"));
    ok("once resolved, Continue in B is enabled", (await buttonState(page, "Continue in B"))?.disabled === false);
    await (await button(page, "Continue in B")).click();
    await sleep(300);
    const c = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_continue"));
    ok("  and continues the rebase inside B", c?.args?.repoPath === "/repos/gitswitch/B");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.subSections, { submodules: SUBMODULES, subInner: SUB_INNER });
    await sleep(300);
    const s = await text(page);
    ok("with no submodule statuses no section renders", !s.includes("Inside A") && !s.includes("Inside B"));
    await page.close();
  }
  {
    // The sync paused inside A: its rebase belongs to the sync, and the backend refuses a plain continue/abort there.
    const page = await open(SCENARIOS.syncPausedInSub, { submoduleStatuses: [SUB_SYNC_PAUSED] });
    await page.waitForFunction(() => document.body.innerText.includes("Inside A"), { timeout: 4000 });
    const s = await text(page);
    ok("a rebase that belongs to the paused sync shows no Continue in A / Abort in A", s.includes("Rebase in progress inside A") && (await button(page, "Continue in A")) === null && (await button(page, "Abort in A")) === null);
    ok("  and points at the paused update's own buttons", s.includes("Finish it with Continue update / Abort update above") && (await button(page, "Abort update")) !== null);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Conflict helpers");
  {
    const page = await open(SCENARIOS.conflicted);
    const s = await text(page);
    ok("a merge conflict says what mine and theirs mean", s.includes("mine = main") && s.includes("theirs = the branch being merged in"));
    const row = await rowWith(page, "conflict.txt");
    ok("each row offers Keep mine and Take theirs, naming the sides", row !== null && row.buttons.some((b) => b.title === "Keep mine (main)") && row.buttons.some((b) => b.title === "Take theirs (the branch being merged in)"));
    ok("  next to Mark resolved", row.buttons.some((b) => b.title === "Mark resolved"));
    await clickInRow(page, "conflict.txt", { title: "Take theirs (the branch being merged in)" });
    await sleep(300);
    const th = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_resolve_side"));
    ok("Take theirs resolves that file with theirs", th?.args?.paths?.length === 1 && th.args.paths[0] === "conflict.txt" && th.args.side === "theirs");
    await (await button(page, "Keep mine for all")).click();
    await sleep(300);
    const all = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_resolve_side")[1]);
    ok("Keep mine for all sends every conflicted path with mine", all?.args?.side === "mine" && all.args.paths.length === 1 && all.args.paths[0] === "conflict.txt");
    await page.close();
  }
  {
    const page = await open(SCENARIOS.rebasingConflicted);
    const s = await text(page);
    ok("in a rebase, mine is the commit being replayed", s.includes("mine = your commit being replayed") && s.includes("theirs = origin/main"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPausedInSub);
    const row = await rowWith(page, "both modified");
    ok("a conflicted gitlink row with no commit named for either side has no side buttons", row !== null && !row.buttons.some((b) => b.title.startsWith("Keep mine") || b.title.startsWith("Take theirs")));
    await page.close();
  }
  {
    // A conflicted submodule pointer whose sides the status names: Keep mine / Take theirs move it there.
    const pointer = {
      ...SCENARIOS.conflicted.entries[0],
      path: "vendor/lib",
      is_submodule: true,
      stage_modes: ["160000", "160000", "160000"],
      stage_oids: ["a".repeat(40), "b".repeat(40), "c".repeat(40)],
      pointer_mine: "b".repeat(40),
      pointer_theirs: "c".repeat(40),
    };
    const page = await open({ ...SCENARIOS.conflicted, entries: [...SCENARIOS.conflicted.entries, pointer], conflicted_count: 2 });
    const row = await rowWith(page, "vendor/lib");
    ok("a conflicted submodule pointer offers Keep mine and Take theirs, naming the commit each records", row !== null && row.buttons.some((b) => b.title.startsWith("Keep mine (main): vendor/lib at bbbbbbb")) && row.buttons.some((b) => b.title.startsWith("Take theirs (the branch being merged in): vendor/lib at ccccccc")), JSON.stringify(row?.buttons));
    await clickInRow(page, "vendor/lib", { text: "Take theirs" });
    await sleep(300);
    const th = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_resolve_side").at(-1));
    ok("  Take theirs resolves that pointer with theirs", th?.args?.paths?.length === 1 && th.args.paths[0] === "vendor/lib" && th.args.side === "theirs", JSON.stringify(th?.args));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.stashConflict);
    const s = await text(page);
    ok("conflicts from a stash say so", s.includes("came from applying a stash") && s.includes("still in the list"));
    ok("  with no operation banner to continue or abort", !s.includes("in progress"));
    const row = await rowWith(page, "conflict.txt");
    ok("  and Discard is the third way out", row !== null && row.buttons.some((b) => b.title === "Discard — this cannot be undone"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Layout");
  // Measure <main>, not documentElement: main has overflow-y-auto, so it
  // scrolls horizontally on its own and the document never reports overflow.
  const longPathStatus = {
    ...SCENARIOS.dirty,
    entries: [
      ...SCENARIOS.dirty.entries,
      { ...SCENARIOS.dirty.entries[2], path: "packages/worker/src/infrastructure/persistence/repositories/very-long-name.ts" },
      { ...SCENARIOS.dirty.entries[2], path: ".seed/gate-receipts/local-e297545bea5310a95a353cc6867072e3cfab5cca.json" },
    ],
  };
  for (const width of [700, 760, 900, 1024, 1280]) {
    const page = await open({ ...longPathStatus, has_submodules: true }, { width, submodules: SUBMODULES, lfs: LFS_POINTERS, submoduleStatuses: SUB_STATUSES, stashes: STASHES });
    await sleep(300);
    const overflow = await page.evaluate(() => {
      const m = document.querySelector("main");
      return { need: m.scrollWidth, have: m.clientWidth };
    });
    ok(`no horizontal overflow at ${width}px, even with long paths`, overflow.need <= overflow.have + 1, `main needs ${overflow.need}px but has ${overflow.have}px`);
    await page.close();
  }
  {
    const page = await open(longPathStatus, { width: 700 });
    const truncated = await page.evaluate(() =>
      [...document.querySelectorAll("span")]
        .filter((s) => s.textContent.includes("very-long-name.ts"))
        .some((s) => s.scrollWidth > s.clientWidth + 1)
    );
    ok("a long file path truncates instead of stretching the layout", truncated);
    ok("the side column is still reachable when narrow", await page.evaluate(() => [...document.querySelectorAll("h3")].some((h) => h.innerText === "Push access")));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.subSections, { width: 700, submodules: [...SUBMODULES, SUBMODULE_A], subInner: SUB_INNER, submoduleStatuses: SUB_STATUSES, stashes: STASHES });
    await page.waitForFunction(() => document.body.innerText.includes("Inside A"), { timeout: 4000 });
    await sleep(300);
    const overflow = await page.evaluate(() => {
      const m = document.querySelector("main");
      return { need: m.scrollWidth, have: m.clientWidth };
    });
    ok("submodule sections don't overflow at 700px", overflow.need <= overflow.have + 1, `main needs ${overflow.need}px but has ${overflow.have}px`);
    const truncated = await page.evaluate(() =>
      [...document.querySelectorAll("span")]
        .filter((s) => s.textContent.includes("long/name.ts"))
        .some((s) => s.scrollWidth > s.clientWidth + 1)
    );
    ok("  and a long path inside a submodule truncates", truncated);
    await page.close();
  }

  section("A fetch flag going stale is not a move: no full-page reload");
  {
    // The page re-reads everything (submodules, workspace, history) when the
    // repository moved — and once when a fetch just happened. The "fetched a
    // moment ago" flag expiring ten seconds later used to count as a move too,
    // so every fetch cost two full reloads, hundreds of git runs each.
    const page = await open({ ...SCENARIOS.submodules, last_fetch_secs: 2 }, { submodules: SUBMODULES, submoduleStatuses: SUB_STATUSES, workspace: WORKSPACE_TWO });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "workspace_info"), { timeout: 4000 });
    await sleep(4200); // past the focus refresh's minimum interval
    const reloads = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "workspace_info").length);
    const statuses = () => page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status" && c.args.repoPath === "/repos/gitswitch").length);
    const before = await reloads();
    const readsBefore = await statuses();
    // The flag has expired by the next read; coming back to the window re-reads the status.
    await page.evaluate(() => { window.__SCENARIO__ = { ...window.__SCENARIO__, last_fetch_secs: 25 }; });
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "changes_repo_status" && c.args.repoPath === "/repos/gitswitch").length > n, { timeout: 4000 }, readsBefore);
    await sleep(600);
    ok("the status was re-read on return", (await statuses()) > readsBefore);
    ok("  but the flag expiring did not reload the whole page", (await reloads()) === before, `workspace_info read ${(await reloads()) - before} more time(s)`);
    // A fetch that just happened is a move worth one reload.
    await sleep(4200);
    await page.evaluate(() => { window.__SCENARIO__ = { ...window.__SCENARIO__, last_fetch_secs: 1 }; });
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await page.waitForFunction((n) => window.__CALLS__.filter((c) => c.cmd === "workspace_info").length > n, { timeout: 4000 }, before).catch(() => null);
    ok("  a fresh fetch reloads the page once", (await reloads()) === before + 1, `workspace_info read ${(await reloads()) - before} more time(s)`);
    await page.close();
  }

  // --- Find repositories --------------------------------------------------
  {
    const at = "2026-01-01T00:00:00Z";
    const prof = (id, name, key, isDefault, over = {}) => ({ id, name, git_name: name, git_email: `${id}@example.test`, ssh_key_path: key, is_default: isDefault, allow_push: true, signing_enabled: false, directories: [], remote_patterns: [], github_login: null, created_at: at, updated_at: at, ...over });
    const PROFILES = [prof("pp", "Personal", "/Users/me/.ssh/id_me", true), prof("pw", "Work", "/Users/me/.ssh/id_work", false, { remote_patterns: ["acme-rules/"] })];
    const fr = (name, over = {}) => {
      const owner = over.owner ?? "x";
      return { path: `/Users/me/code/${name}`, remote_url: owner ? `git@github.com:${owner}/${name}.git` : "", owner, name: owner ? name : "", push_owner: owner, push_name: owner ? name : "", gitdir_elsewhere: false, commits_as: { profile_id: "pp", via: "default" }, ...over };
    };
    const FOUND = {
      home: "/Users/me",
      roots: ["/clones", "/w", "/Users/me"],
      already_listed: 3,
      truncated: [],
      unreadable: [],
      rejected: [],
      linked: [],
      cancelled: false,
      repos: [
        fr("feature-wt", { owner: "me-home", gitdir_elsewhere: true }),
        fr("local-only", { owner: "" }),
        fr("mine", { owner: "me-home" }),
        fr("public-lib", { owner: "vercel" }),
        fr("ruled", { owner: "acme-rules", commits_as: { profile_id: "pw", via: "rule" } }),
        fr("shared", { owner: "orgx" }),
        fr("sso", { owner: "bigorg" }),
        fr("tool", { owner: "acme" }),
      ],
    };
    const V = (push, priv) => ({ state: "visible", push, admin: false, private: priv, why: null });
    const INV = { state: "invisible", push: false, admin: false, private: null, why: null };
    const MATRIX = {
      accounts: [{ login: "Me-Home", source: "GitHub CLI" }, { login: "me-work", source: "GitHub CLI" }],
      problems: [],
      access: {
        "me-home/mine": { "me-home": V(true, true), "me-work": INV },
        "acme/tool": { "me-home": INV, "me-work": V(true, true) },
        "vercel/public-lib": { "me-home": V(false, false), "me-work": V(false, false) },
        "bigorg/sso": { "me-home": INV, "me-work": { state: "unknown", push: false, admin: false, private: null, why: "single sign-on: the token is not authorized for this organization" } },
        "orgx/shared": { "me-home": V(true, false), "me-work": V(true, false) },
      },
      cancelled: false,
    };
    const FIND = { result: FOUND, profiles: PROFILES, keyLogins: { "/Users/me/.ssh/id_me": "Me-Home", "/Users/me/.ssh/id_work": "me-work" }, matrix: MATRIX, hints: { orgx: "pw" } };
    const calls = (page, cmd) => page.evaluate((cmd) => window.__CALLS__.filter((c) => c.cmd === cmd).map((c) => c.args), cmd);
    const rowText = (page, name) => page.evaluate((name) => [...document.querySelectorAll("[data-find-row]")].find((r) => r.dataset.findRow.endsWith(`/${name}`))?.innerText ?? "", name);
    const rowNames = (page) => page.evaluate(() => [...document.querySelectorAll("[data-find-row]")].map((r) => r.dataset.findRow.split("/").pop()));
    const openFind = async (page) => {
      await (await button(page, "Find repositories"))?.click();
      await page.waitForFunction(() => document.body.innerText.includes("Looking in your home folder"), { timeout: 4000 });
    };
    const closeFind = (page) => page.evaluate(() => [...document.querySelectorAll("h2")].find((h) => h.innerText === "Find repositories")?.parentElement.querySelector("button")?.click());
    /** A button inside the Find repositories dialog, by its label (exact, else the first starting with it). */
    const findButton = (page, label, press) =>
      page.evaluate(
        (label, press) => {
          const box = [...document.querySelectorAll("h2")].find((h) => h.innerText === "Find repositories")?.closest(".relative");
          const all = box ? [...box.querySelectorAll("button")] : [];
          const b = all.find((x) => x.innerText.trim() === label) ?? all.find((x) => x.innerText.trim().startsWith(label));
          if (b && press) b.click();
          return !!b;
        },
        label,
        press
      );
    const press = (page, label) => findButton(page, label, true);
    const has = (page, label) => findButton(page, label, false);
    const waitRows = (page, n) => page.waitForFunction((n) => document.querySelectorAll("[data-find-row]").length === n, { timeout: 6000 }, n).catch(() => null);

    section("Find repositories: clones no profile's folders hold, each proposed to the profile whose account can reach it");
    {
      const page = await open(SCENARIOS.clean, { find: FIND, storage: { "sparse.parentDir": "/clones", "autoassign.root": "/w" } });
      ok("the Repository page has a Find repositories button", !!(await button(page, "Find repositories")));
      await openFind(page);
      await sleep(300);
      ok("the first time, it says what it will read and searches only when asked", (await has(page, "Search")) && (await calls(page, "find_unlisted_repos")).length === 0, await text(page));
      await press(page, "Search");
      await waitRows(page, 8);
      const args = (await calls(page, "find_unlisted_repos"))[0] ?? {};
      ok("  the search takes the Clone page's and Auto Assign's remembered folders", JSON.stringify(args.extraRoots) === JSON.stringify(["/clones", "/w"]) && !!args.scanId, JSON.stringify(args));
      ok("  GitHub is asked about each origin once", JSON.stringify(((await calls(page, "repo_access_matrix"))[0] ?? {}).repos?.slice().sort()) === JSON.stringify(["acme-rules/ruled", "acme/tool", "bigorg/sso", "me-home/mine", "orgx/shared", "vercel/public-lib"]), JSON.stringify(await calls(page, "repo_access_matrix")));
      const mine = await rowText(page, "mine");
      ok("a repository only one profile's account can push to is proposed to that profile", mine.includes("push · Me-Home") && mine.includes("Personal (Me-Home)"), mine);
      const tool = await rowText(page, "tool");
      ok("  and the row says whose email it commits with now, and after", tool.includes("push · me-work") && tool.includes("Commits as Personal · after adding: Work"), tool);
      const pub = await rowText(page, "public-lib");
      ok("a public repository nobody can push to is not proposed", pub.includes("anyone can read it") && pub.includes("Pick a profile"), pub);
      const sso = await rowText(page, "sso");
      ok("a check GitHub could not answer proposes nothing and says why", sso.includes("check failed · me-work: single sign-on") && sso.includes("Pick a profile"), sso);
      ok("a repository that is not on GitHub says so", (await rowText(page, "local-only")).includes("not on GitHub"));
      const ruled = await rowText(page, "ruled");
      ok("a clone a remote rule already gives a profile is proposed to that profile", ruled.includes("Work's remote rule") && ruled.includes("Work (me-work)"), ruled);
      const wt = await rowText(page, "feature-wt");
      ok("a worktree is offered only the profile its git folder already gives it", wt.includes("a worktree: it commits as Personal") && wt.includes("Personal"), wt);
      const shared = await rowText(page, "shared");
      ok("an organisation's repository several profiles can push to: its other clones' profile is preselected", shared.includes("several profiles can push · your other orgx clones use Work") && shared.includes("Work (me-work)"), shared);
      const fits = await page.evaluate(() => {
        const box = [...document.querySelectorAll("h2")].find((h) => h.innerText === "Find repositories")?.closest(".relative");
        const rows = [...document.querySelectorAll("[data-find-row]")];
        return !!box && box.getBoundingClientRect().right <= window.innerWidth && rows.every((r) => r.scrollWidth <= r.clientWidth + 1);
      });
      ok("  the dialog and its rows fit the window", fits);
      const listedBefore = (await calls(page, "history_list_repos")).length;
      await press(page, "Add 5 repositories to their profiles");
      await page.waitForFunction(() => document.body.innerText.includes("Added 5 repositories"), { timeout: 4000 }).catch(() => null);
      const added = (await calls(page, "profile_folders_add"))[0]?.changes ?? [];
      ok(
        "Add puts each chosen repository under its profile, and nothing else",
        JSON.stringify(added.map((c) => `${c.path.split("/").pop()}:${c.profile_id}`)) === JSON.stringify(["feature-wt:pp", "mine:pp", "ruled:pw", "shared:pw", "tool:pw"]),
        JSON.stringify(added)
      );
      ok("  the Repository page reads its list again", (await calls(page, "history_list_repos")).length > listedBefore);
      ok("  what was not added stays for review", JSON.stringify(await rowNames(page)) === JSON.stringify(["local-only", "public-lib", "sso"]), JSON.stringify(await rowNames(page)));
      await press(page, "Undo");
      await sleep(400);
      const removed = (await calls(page, "profile_folders_remove"))[0]?.changes ?? [];
      ok("Undo takes exactly those off again", removed.length === 5 && removed.every((c) => added.some((a) => a.path === c.path && a.profile_id === c.profile_id)), JSON.stringify(removed));
      await page.close();
    }

    section("Find repositories: clear matches added straight away, with Undo after closing");
    {
      const page = await open(SCENARIOS.clean, { find: FIND, storage: { "findrepos.autoAdd": true, "findrepos.searched": true } });
      await openFind(page);
      await page.waitForFunction(() => document.body.innerText.includes("Added 4 repositories"), { timeout: 6000 }).catch(() => null);
      const added = (await calls(page, "profile_folders_add"))[0]?.changes ?? [];
      ok("once searched before, opening searches at once", (await calls(page, "find_unlisted_repos")).length === 1);
      ok("  the clear ones are added without a click — and the preselected guess is not", JSON.stringify(added.map((c) => c.path.split("/").pop())) === JSON.stringify(["feature-wt", "mine", "ruled", "tool"]), JSON.stringify(added));
      ok("  the rest wait for review", JSON.stringify(await rowNames(page)) === JSON.stringify(["local-only", "public-lib", "shared", "sso"]), JSON.stringify(await rowNames(page)));
      await closeFind(page);
      await sleep(300);
      await openFind(page);
      await sleep(400);
      ok("opened again, it shows what was added with Undo, and does not search over it", (await text(page)).includes("Added 4 repositories") && (await has(page, "Undo")) && (await calls(page, "find_unlisted_repos")).length === 1);
      await press(page, "Undo");
      await sleep(400);
      ok("  Undo takes those four off", ((await calls(page, "profile_folders_remove"))[0]?.changes ?? []).length === 4);
      await page.close();
    }

    section("Find repositories: closing while it searches adds nothing");
    {
      const page = await open(SCENARIOS.clean, { find: { ...FIND, hold: true }, storage: { "findrepos.autoAdd": true, "findrepos.searched": true } });
      await openFind(page);
      await page.waitForFunction(() => typeof window.__RELEASE_FIND__ === "function", { timeout: 4000 }).catch(() => null);
      await page.waitForFunction(() => document.body.innerText.includes("1,536 folders"), { timeout: 3000 }).catch(() => null);
      ok("while it looks, it says how far it has got", (await text(page)).includes("Looking for repositories · 1,536 folders"), await text(page));
      await closeFind(page);
      await sleep(200);
      ok("closing stops the search", (await calls(page, "find_repos_cancel")).length === 1);
      await page.evaluate(() => window.__RELEASE_FIND__());
      await sleep(600);
      ok("  and what it found after that is not added", (await calls(page, "profile_folders_add")).length === 0 && (await calls(page, "repo_access_matrix")).length === 0);
      await page.close();
    }

    section("Find repositories: nothing new, and the places it could not search");
    {
      const NONE = { ...FOUND, repos: [], already_listed: 7 };
      const page = await open(SCENARIOS.clean, { find: { ...FIND, result: NONE, pick: "/extra/place" }, storage: { "findrepos.searched": true } });
      await openFind(page);
      await page.waitForFunction(() => document.body.innerText.includes("already on the Repository page"), { timeout: 4000 }).catch(() => null);
      ok("when every repository is listed already, it says so with the count", (await text(page)).includes("Every repository in these places is already on the Repository page (7)."));
      await page.evaluate(() => {
        window.__FOUND__ = {
          home: "/Users/me", roots: ["/Users/me"], repos: [], already_listed: 0, cancelled: false,
          truncated: [{ place: "/Users/me", why: "stopped after 30 seconds" }],
          unreadable: ["/Users/me/Desktop"],
          rejected: [{ place: "relative/x", why: "not a full path" }],
          linked: [{ from: "/Users/me/code", to: "/Volumes/SSD/code" }],
        };
      });
      await press(page, "Search again");
      await page.waitForFunction(() => document.body.innerText.includes("Look there too"), { timeout: 4000 }).catch(() => null);
      const t2 = await text(page);
      ok("a place cut short says where and why", t2.includes("~: stopped after 30 seconds"), t2);
      ok("  a folder it could not read is named", t2.includes("Could not read ~/Desktop"), t2);
      ok("  a place asked for that is not usable says why", t2.includes("Not searched: relative/x (not a full path)"), t2);
      ok("  a linked folder that leads elsewhere is offered", t2.includes("~/code is a link to /Volumes/SSD/code, which was not searched."), t2);
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Look there too")?.click());
      await sleep(500);
      const last = (await calls(page, "find_unlisted_repos")).pop() ?? {};
      ok("Look there too searches it, and remembers it", (last.extraRoots ?? []).includes("/Volumes/SSD/code") && (await page.evaluate(() => localStorage.getItem("gitswitch:findrepos.places") ?? "")).includes("/Volumes/SSD/code"), JSON.stringify(last));
      await press(page, "Also look in…");
      await sleep(500);
      const again = (await calls(page, "find_unlisted_repos")).pop() ?? {};
      ok("Also look in… adds the folder picked and searches again", (again.extraRoots ?? []).includes("/extra/place"), JSON.stringify(again));
      await page.close();
    }

    section("Find repositories: tick what to add — select all, clear matches, filter chips, search, one profile for the ticked ones, hide");
    {
      const NINE = { ...FIND, result: { ...FOUND, repos: [...FOUND.repos, fr("zeta-extra", { owner: "me-home" })] } };
      const page = await open(SCENARIOS.clean, { find: NINE, storage: { "findrepos.searched": true } });
      await openFind(page);
      await waitRows(page, 9);
      const picked = () => page.evaluate(() => document.querySelector("[data-find-picked]")?.innerText ?? "");
      const tick = (name) => page.evaluate((name) => document.querySelector(`input[aria-label="Select ${name}"]`)?.click(), name);
      const chip = (k) => page.evaluate((k) => document.querySelector(`[data-find-chip="${k}"]`)?.click(), k);
      const chipText = (k) => page.evaluate((k) => document.querySelector(`[data-find-chip="${k}"]`)?.innerText ?? "", k);
      ok("rows with a proposed profile start ticked; the count says so", (await picked()) === "5 of 9 selected", await picked());
      ok("  the chips count clear matches, rows needing a choice and rows not on GitHub", (await chipText("clear")) === "Clear match 4" && (await chipText("offline")) === "Not on GitHub 1" && (await chipText("choose")) === "Needs a choice 4", [await chipText("clear"), await chipText("choose"), await chipText("offline")].join(" | "));
      await chip("offline");
      await sleep(150);
      ok("a chip shows only its rows", JSON.stringify(await rowNames(page)) === JSON.stringify(["local-only"]), JSON.stringify(await rowNames(page)));
      await chip("all");
      await press(page, "Select none");
      await sleep(150);
      ok("Select none unticks everything, and Add has nothing to add", (await picked()) === "0 of 9 selected" && (await has(page, "Add 0 repositories to their profiles")));
      await press(page, "Select clear matches");
      await sleep(150);
      ok("Select clear matches ticks exactly the clear ones", (await picked()) === "4 of 9 selected", await picked());
      await page.evaluate(() => document.querySelector('input[aria-label="Select all"]')?.click());
      await sleep(150);
      ok("Select all ticks every row", (await picked()) === "9 of 9 selected", await picked());
      await page.evaluate(() => document.querySelector('input[aria-label="Select all"]')?.click());
      await sleep(150);
      ok("  and unticks them again", (await picked()) === "0 of 9 selected", await picked());

      const filterBox = await page.$('input[aria-label="Filter repositories"]');
      ok("with more than eight rows there is a filter box", !!filterBox);
      await filterBox?.type("zeta");
      await sleep(150);
      ok("  typing filters the rows by name, folder or owner", JSON.stringify(await rowNames(page)) === JSON.stringify(["zeta-extra"]), JSON.stringify(await rowNames(page)));
      await page.evaluate(() => document.querySelector('input[aria-label="Select all shown"]')?.click());
      await sleep(150);
      ok("  Select all then ticks only the rows shown", (await picked()) === "1 of 9 selected", await picked());
      await filterBox?.click({ clickCount: 3 });
      await page.keyboard.press("Backspace");
      await sleep(150);

      await tick("feature-wt");
      await tick("public-lib");
      await sleep(150);
      ok("ticking rows adds them to the count", (await picked()) === "3 of 9 selected", await picked());
      ok("  a ticked row with no profile says it will be skipped, and so does the footer", (await rowText(page, "public-lib")).includes("Pick a profile for it, or it is skipped") && (await text(page)).includes("2 selected have no profile and are skipped"), await rowText(page, "public-lib"));
      await page.evaluate(() => [...document.querySelectorAll("[data-find-bulk] button")].find((b) => b.innerText.trim() === "Pick a profile")?.click());
      await sleep(150);
      await page.evaluate(() => [...document.querySelectorAll("[data-find-bulk] button")].find((b) => b.innerText.trim().startsWith("Work"))?.click());
      await sleep(200);
      const note = await page.evaluate(() => document.querySelector("[data-find-bulk] [role=status]")?.innerText ?? "");
      ok("one profile for the ticked rows: set where it can be, kept where git already ties the row to another", note.includes("2 repositories set to Work") && note.includes("1 kept the profile git already uses for it: feature-wt"), note);
      ok("  the rows show it", (await rowText(page, "public-lib")).includes("Work (me-work)") && (await rowText(page, "feature-wt")).includes("Personal"), await rowText(page, "public-lib"));

      await page.evaluate(() => document.querySelector('button[aria-label="Hide sso"]')?.click());
      await sleep(200);
      ok("Hide takes a row off the list, and remembers it", !(await rowNames(page)).includes("sso") && (await page.evaluate(() => localStorage.getItem("gitswitch:findrepos.hidden") ?? "")).includes("/sso") && (await text(page)).toLowerCase().includes("1 hidden"), JSON.stringify(await rowNames(page)));
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "show them")?.click());
      await sleep(150);
      ok("  show them brings it back greyed, with Show again", (await rowNames(page)).includes("sso") && (await rowText(page, "sso")).includes("Show again"));

      await press(page, "Add 3 repositories to their profiles");
      await page.waitForFunction(() => document.body.innerText.includes("Added 3 repositories"), { timeout: 4000 }).catch(() => null);
      const added = ((await calls(page, "profile_folders_add"))[0]?.changes ?? []).map((c) => `${c.path.split("/").pop()}:${c.profile_id}`);
      ok("Add takes only the ticked rows that have a profile", JSON.stringify(added) === JSON.stringify(["feature-wt:pp", "public-lib:pw", "zeta-extra:pw"]), JSON.stringify(added));
      await page.close();

      const again = await open(SCENARIOS.clean, { find: FIND, storage: { "findrepos.searched": true, "findrepos.autoAdd": true, "findrepos.hidden": ["/Users/me/code/mine"] } });
      await openFind(again);
      await again.waitForFunction(() => document.body.innerText.includes("Added 3 repositories"), { timeout: 6000 }).catch(() => null);
      const auto = ((await calls(again, "profile_folders_add"))[0]?.changes ?? []).map((c) => c.path.split("/").pop());
      ok("a hidden row is never added straight away", JSON.stringify(auto) === JSON.stringify(["feature-wt", "ruled", "tool"]), JSON.stringify(auto));
      await again.close();
    }

    section("Find repositories: the ones a profile's folder already holds are shown too, labelled with the profile and its account");
    {
      const LISTED = [
        { path: "/Users/me/work/api", profile_id: "pw", profile_name: "Work", github_login: null },
        { path: "/Users/me/code/site", profile_id: "pp", profile_name: "Personal", github_login: "Me-Home" },
      ];
      const page = await open(SCENARIOS.clean, { find: { ...FIND, result: { ...FOUND, already_listed: 2, listed: LISTED } }, storage: { "findrepos.searched": true } });
      await openFind(page);
      await waitRows(page, 8);
      const added = () => page.evaluate(() => [...document.querySelectorAll("[data-find-listed]")].map((r) => r.innerText.replace(/\s+/g, " ").trim()));
      const a1 = await added();
      ok("they are listed after the others, each saying which profile and account it is with", JSON.stringify(a1) === JSON.stringify(["api · ~/work/api Already in Work (me-work)", "site · ~/code/site Already in Personal (Me-Home)"]), JSON.stringify(a1));
      ok("  they take no part in choosing: no checkbox, no picker, not counted", (await page.$$('[data-find-listed] input, [data-find-listed] button')).length === 0 && (await page.evaluate(() => document.querySelector("[data-find-picked]")?.innerText)) === "5 of 8 selected");
      ok("  a chip shows only them", (await page.evaluate(() => document.querySelector('[data-find-chip="added"]')?.innerText)) === "Already added 2");
      await page.evaluate(() => document.querySelector('[data-find-chip="added"]')?.click());
      await sleep(150);
      ok("  with nothing else, and no 'Nothing here matches'", (await rowNames(page)).length === 0 && (await added()).length === 2 && !(await text(page)).includes("Nothing here matches"));
      await page.evaluate(() => document.querySelector('[data-find-chip="clear"]')?.click());
      await sleep(150);
      ok("  another chip leaves them out", (await added()).length === 0);
      await press(page, "Add 5 repositories to their profiles");
      await page.waitForFunction(() => document.body.innerText.includes("Added 5 repositories"), { timeout: 4000 }).catch(() => null);
      ok("adding is as before: the five chosen, none of these", ((await calls(page, "profile_folders_add"))[0]?.changes ?? []).length === 5);
      await page.close();

      const all = await open(SCENARIOS.clean, { find: { ...FIND, result: { ...FOUND, repos: [], already_listed: 2, listed: LISTED } }, storage: { "findrepos.searched": true } });
      await openFind(all);
      await all.waitForFunction(() => document.querySelectorAll("[data-find-listed]").length === 2, { timeout: 4000 }).catch(() => null);
      const t = await text(all);
      ok("when every repository is added already, it says so and lists them, with the account the profile names", t.includes("Every repository in these places is already on the Repository page (2).") && t.includes("Already in Personal (Me-Home)") && t.includes("Already in Work"), t.slice(0, 900));
      await all.close();
    }
  }

  // --- GitHub: pull requests, review & merge, collaborators -----------------
  {
    const PR = (n, state, extra = {}) => ({
      repo: "acme/app", number: n, title: `PR ${n}`, author: "me-home", draft: false, state, created_at: "2026-10-01T00:00:00Z", updated_at: "2026-10-30T00:00:00Z",
      merged_at: state === "merged" ? "2026-10-20T00:00:00Z" : null, closed_at: state === "open" ? null : "2026-10-20T00:00:00Z", head_ref: `feat/${n}`, base_ref: "main",
      comments: null, labels: [], html_url: `https://github.com/acme/app/pull/${n}`, ...extra,
    });
    const DETAIL = {
      repo: "acme/app", number: 1, title: "PR 1", body: "Adds the thing.", author: "me-home", state: "open", draft: false, merged_by: null, merged_at: null,
      mergeable: true, mergeable_state: "clean", head_ref: "feat/1", head_sha: "abc1234def5678", head_repo: "acme/app", base_ref: "main", default_branch: "main",
      additions: 3, deletions: 1, changed_files: 2, commits: 2, comments: 1, created_at: "2026-10-01T00:00:00Z", updated_at: "2026-10-30T00:00:00Z",
      html_url: "https://github.com/acme/app/pull/1", merge_methods: ["merge", "squash"], merge_methods_known: true, can_push: true, is_admin: true, viewer: "work-me",
      reviews: [{ login: "pal", state: "APPROVED", submitted_at: "2026-10-29T00:00:00Z" }], checks: [{ name: "build", state: "success", url: null }, { name: "lint", state: "failure", url: null }],
      notes: [], can_delete_branch: true,
    };
    const GH = {
      context: {
        owner: "acme", name: "app", not_github: null, profile_login: "work-me", suggested: "gh:work-me",
        accounts: [
          { account: "gh:me-home", login: "me-home", source: "GitHub CLI", permission: "read", private: true, why: null },
          { account: "gh:work-me", login: "work-me", source: "GitHub CLI", permission: "admin", private: true, why: null },
        ],
      },
      accounts: [{ key: "gh:me-home", label: "me-home", source: "GitHub CLI" }, { key: "gh:work-me", label: "work-me", source: "GitHub CLI" }],
      pulls: [PR(1, "open"), PR(2, "open", { draft: true, title: "Draft thing" }), PR(3, "merged"), PR(4, "closed"), PR(5, "merged", { title: "Old fix" })],
      search: [PR(9, "open", { repo: "other/lib", title: "Elsewhere", head_ref: null, base_ref: null, comments: 2, author: "pal" }), PR(10, "merged", { repo: "acme/app", title: "Mine merged", head_ref: null, base_ref: null })],
      detail: DETAIL,
      details: { 2: { draft: true, title: "Draft thing", head_ref: "feat/2" } },
      files: { truncated: false, files: [
        { path: "src/a.txt", previous_path: null, status: "modified", additions: 1, deletions: 1, diff: { path: "src/a.txt", staged: false, lines: [{ kind: "hunk", text: "@@ -1 +1 @@" }, { kind: "del", text: "-old line" }, { kind: "add", text: "+new line" }], is_binary: false, truncated: false, total_lines: 3, added: 1, removed: 1, empty_reason: null } },
        { path: "logo.png", previous_path: null, status: "added", additions: 0, deletions: 0, diff: { path: "logo.png", staged: false, lines: [], is_binary: false, truncated: false, total_lines: 0, added: 0, removed: 0, empty_reason: "GitHub shows no diff for this file (binary, or too large)." } },
      ] },
      commits: [{ sha: "c1aaaaaaaa", title: "first", author: "me-home", date: null }, { sha: "c2bbbbbbbb", title: "second", author: "pal", date: null }],
      people: { collaborators: [{ login: "acme", role: "admin", is_owner: true }, { login: "pal", role: "write", is_owner: false }], invitations: [{ id: 77, login: "newbie", role: "write", created_at: "2026-10-30T00:00:00Z", inviter: "work-me" }], note: null },
    };
    const gcalls = (page, cmd) => page.evaluate((cmd) => window.__CALLS__.filter((c) => c.cmd === cmd).map((c) => c.args), cmd);
    const pullRows = (page) => page.evaluate(() => [...document.querySelectorAll("[data-pull-row]")].map((r) => r.dataset.pullRow));
    const clickText = (page, sel, label) => page.evaluate((sel, label) => { const b = [...document.querySelectorAll(sel)].find((x) => x.innerText.trim() === label); b?.click(); return !!b; }, sel, label);
    const pill = (page, label) => clickText(page, "[role=group] button", label);
    const inPanel = (page, label) => page.evaluate((label) => { const d = document.querySelector('[role=dialog][aria-label^="Pull request"]'); const b = d && [...d.querySelectorAll("button")].find((x) => x.innerText.trim() === label); if (b && !b.disabled) b.click(); return b ? (b.disabled ? "disabled" : "clicked") : "missing"; }, label);
    const panelText = (page) => page.evaluate(() => document.querySelector('[role=dialog][aria-label^="Pull request"]')?.innerText ?? "");
    const openTab = async (page) => {
      await page.evaluate(() => [...document.querySelectorAll('[role=tab]')].find((t) => t.innerText.trim() === "GitHub")?.click());
      await page.waitForFunction(() => document.querySelectorAll("[data-pull-row]").length > 0, { timeout: 5000 }).catch(() => null);
    };

    section("GitHub tab: the open repository's pull requests, as the profile's account — filters, Load more, Refresh, another account");
    {
      const page = await open(SCENARIOS.clean, { gh: GH });
      ok("the Repository page has a GitHub tab", await page.evaluate(() => [...document.querySelectorAll('[role=tab]')].some((t) => t.innerText.trim() === "GitHub")));
      ok("  nothing is asked of GitHub before the tab is opened", (await gcalls(page, "github_repo_context")).length === 0);
      await openTab(page);
      ok("opening it asks which account can act on the open repository", JSON.stringify((await gcalls(page, "github_repo_context"))[0]) === JSON.stringify({ repoPath: "/repos/gitswitch", repo: null }), JSON.stringify(await gcalls(page, "github_repo_context")));
      const t = await text(page);
      ok("  it acts as the profile's own account, and says what it may do", t.includes("acme/app") && t.includes("work-me · admin · this profile"), t.slice(0, 600));
      ok("  the open pull requests, as that account", JSON.stringify((await gcalls(page, "github_pulls"))[0]) === JSON.stringify({ account: "gh:work-me", owner: "acme", name: "app", state: "open", page: 1 }) && JSON.stringify(await pullRows(page)) === JSON.stringify(["acme/app#1", "acme/app#2"]), JSON.stringify(await gcalls(page, "github_pulls")));
      ok("  a draft says so", (await page.evaluate(() => document.querySelector('[data-pull-row="acme/app#2"]')?.innerText ?? "")).includes("Draft"));
      await pill(page, "Merged");
      await sleep(250);
      ok("Merged: only the merged ones", (await gcalls(page, "github_pulls")).at(-1)?.state === "merged" && JSON.stringify(await pullRows(page)) === JSON.stringify(["acme/app#3", "acme/app#5"]), JSON.stringify(await pullRows(page)));
      await pill(page, "Closed");
      await sleep(250);
      ok("Closed: the ones closed without merging", JSON.stringify(await pullRows(page)) === JSON.stringify(["acme/app#4"]), JSON.stringify(await pullRows(page)));
      await pill(page, "All");
      await sleep(250);
      ok("All: the first page, and Load more", (await pullRows(page)).length === 2 && !!(await button(page, "Load more")));
      await (await button(page, "Load more"))?.click();
      await sleep(250);
      ok("  Load more asks for the next page and adds it", (await gcalls(page, "github_pulls")).at(-1)?.page === 2 && (await pullRows(page)).length === 4, JSON.stringify(await pullRows(page)));
      await page.type('input[aria-label="Filter pull requests"]', "old fix");
      await sleep(150);
      ok("the filter box narrows what is loaded", JSON.stringify(await pullRows(page)) === JSON.stringify([]) || JSON.stringify(await pullRows(page)) === JSON.stringify(["acme/app#5"]), JSON.stringify(await pullRows(page)));
      await page.click('input[aria-label="Filter pull requests"]', { clickCount: 3 });
      await page.keyboard.press("Backspace");
      const before = (await gcalls(page, "github_pulls")).length;
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Refresh" && b.title === "Read the pull requests again")?.click());
      await sleep(250);
      ok("Refresh reads the list again", (await gcalls(page, "github_pulls")).length === before + 1);
      ok("  the state picked is remembered", (await page.evaluate(() => localStorage.getItem("gitswitch:github.pullState"))) === '"all"');
      await pill(page, "Open");
      await sleep(200);
      // Act as the other account.
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("work-me · admin"))?.click());
      await sleep(100);
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("me-home · can read"))?.click());
      await sleep(250);
      ok("another account picked: the list is asked as it, and the pick is remembered for this repository", (await gcalls(page, "github_pulls")).at(-1)?.account === "gh:me-home" && (await page.evaluate(() => localStorage.getItem("gitswitch:github.actingAs") ?? "")).includes('"acme/app":"gh:me-home"'), JSON.stringify((await gcalls(page, "github_pulls")).at(-1)));
      await page.close();

      const notGh = await open(SCENARIOS.clean, { gh: { ...GH, context: { owner: null, name: null, not_github: "Its remote (https://gitlab.com/x/y.git) is not on GitHub.", accounts: [], suggested: null, profile_login: null } } });
      await notGh.evaluate(() => [...document.querySelectorAll('[role=tab]')].find((t) => t.innerText.trim() === "GitHub")?.click());
      await sleep(400);
      ok("a repository whose remote is not on GitHub says so, and asks nothing more", (await text(notGh)).includes("is not on GitHub. Pull requests and collaborators are for repositories on GitHub.") && (await gcalls(notGh, "github_pulls")).length === 0);
      await notGh.close();
    }

    section("Pull request: files with diffs, commits, checks and reviews; a review; merging with the reviewed commit");
    {
      const page = await open(SCENARIOS.clean, { gh: GH });
      await openTab(page);
      await page.evaluate(() => document.querySelector('[data-pull-row="acme/app#1"]')?.click());
      await page.waitForFunction(() => (document.querySelector('[role=dialog][aria-label^="Pull request"]')?.innerText ?? "").includes("No conflicts"), { timeout: 5000 }).catch(() => null);
      let pt = await panelText(page);
      ok("a row opens the pull request, as the account acting", JSON.stringify((await gcalls(page, "github_pull"))[0]) === JSON.stringify({ account: "gh:work-me", owner: "acme", name: "app", number: 1 }));
      ok("  what it merges, where, and its size", pt.includes("me-home wants to merge 2 commits into main from feat/1") && pt.includes("+3") && pt.includes("−1"), pt.slice(0, 400));
      ok("  whether it can be merged, its checks and its reviews", pt.includes("No conflicts with the base branch") && pt.toLowerCase().includes("checks · 1 of 2 passed") && /pal\s+approved/.test(pt) && pt.includes("Adds the thing."), pt.slice(200, 1200));
      ok("  the files are read only when asked for", (await gcalls(page, "github_pull_files")).length === 0);
      await clickText(page, '[role=dialog] [role=tab]', "Files (2)");
      await page.waitForFunction(() => document.querySelectorAll("[data-pull-file]").length === 2, { timeout: 4000 }).catch(() => null);
      ok("Files: each changed file with its size", (await gcalls(page, "github_pull_files")).length === 1 && (await panelText(page)).includes("2 files changed"));
      await page.evaluate(() => document.querySelector('[data-pull-file="src/a.txt"] button')?.click());
      await sleep(150);
      pt = await panelText(page);
      ok("  a file opens to its diff", pt.includes("-old line") && pt.includes("+new line"), pt.slice(0, 900));
      await page.evaluate(() => [...document.querySelectorAll("[role=dialog] button")].find((b) => b.innerText.trim() === "Expand all")?.click());
      await sleep(150);
      ok("  Expand all opens every file, and a file without a diff says why", (await panelText(page)).includes("GitHub shows no diff for this file (binary, or too large)."));
      ok("  one Unified / Side by side choice above the files, none repeated per file", await page.evaluate(() => document.querySelectorAll("[role=dialog] [data-diff-toggle]").length === 1 && document.querySelectorAll("[data-pull-file] [data-diff-toggle]").length === 0));
      await page.evaluate(() => [...document.querySelectorAll("[role=dialog] [data-diff-toggle] button")].find((b) => b.innerText.trim() === "Side by side")?.click());
      await sleep(150);
      ok("  Side by side shows each file's diff as a table, old left and new right", await page.evaluate(() => { const r = document.querySelector('[role=dialog] [data-diff-view="split"] tr[data-split-row="pair"]'); return !!r && r.querySelector('[data-side="old"]').innerText === "old line" && r.querySelector('[data-side="new"]').innerText === "new line" && r.dataset.old === "1" && r.dataset.new === "1"; }));
      if (process.env.UI_SHOTS) await page.screenshot({ path: `${process.env.UI_SHOTS}/pull-files-side-by-side.png` });
      await page.evaluate(() => [...document.querySelectorAll("[role=dialog] [data-diff-toggle] button")].find((b) => b.innerText.trim() === "Unified")?.click());
      await sleep(150);
      ok("  on Files, review and merge are folded into one bar, so the diffs get the height", await page.evaluate(() => !!document.querySelector('[data-pull-actions="folded"]') && !document.querySelector('textarea[aria-label="Review comment"]')));
      ok("  a file's diff scrolls with the page, not in a little box of its own", await page.evaluate(() => { const d = document.querySelector("[data-pull-diff]"); const cs = d && getComputedStyle(d); return !!cs && cs.maxHeight === "none" && cs.overflowY === "visible"; }));
      await page.setViewport({ width: 900, height: 650 });
      await sleep(200);
      const room = await page.evaluate(() => Math.round(document.querySelector("[role=dialog] .overflow-y-auto.flex-1")?.getBoundingClientRect().height ?? 0));
      ok("  in the app's own window size (900×650) the files get most of it", room >= 330, `${room}px`);
      await page.evaluate(() => [...document.querySelectorAll('[data-pull-actions="folded"] button')].find((b) => b.innerText.includes("Review or merge"))?.click());
      await sleep(150);
      ok("  Review or merge opens them right there, on Files", await page.evaluate(() => !!document.querySelector('[data-pull-actions="open"]') && !!document.querySelector('textarea[aria-label="Review comment"]') && document.querySelector('[role=dialog] [role=tab][aria-selected="true"]')?.innerText.startsWith("Files")));
      await page.evaluate(() => [...document.querySelectorAll('[data-pull-actions="open"] button')].find((b) => b.innerText.includes("Hide review and merge"))?.click());
      await sleep(150);
      ok("  and Hide folds them away again", await page.evaluate(() => !!document.querySelector('[data-pull-actions="folded"]')));
      await page.setViewport({ width: 1280, height: 900 });
      await sleep(150);
      await clickText(page, '[role=dialog] [role=tab]', "Commits (2)");
      await sleep(250);
      ok("Commits: each with its first line and author", (await panelText(page)).includes("first") && (await panelText(page)).includes("c2bbbbb"));

      await page.type('textarea[aria-label="Review comment"]', "Nice work");
      await inPanel(page, "Comment");
      await sleep(250);
      ok("a comment is sent as a review, with its words", JSON.stringify((await gcalls(page, "github_pull_review")).at(-1)) === JSON.stringify({ account: "gh:work-me", owner: "acme", name: "app", number: 1, event: "COMMENT", body: "Nice work" }), JSON.stringify(await gcalls(page, "github_pull_review")));
      ok("  Request changes waits for words", (await inPanel(page, "Request changes")) === "disabled");

      // Merge: squash, delete the branch.
      await page.evaluate(() => { const d = document.querySelector('[role=dialog][aria-label^="Pull request"]'); [...d.querySelectorAll("section[aria-label=Merge] button")].find((b) => b.innerText.includes("Create a merge commit"))?.click(); });
      await sleep(100);
      await page.evaluate(() => [...document.querySelectorAll("section[aria-label=Merge] button")].find((b) => b.innerText.trim() === "Squash and merge")?.click());
      await page.evaluate(() => document.querySelector('input[aria-label="Delete the branch after merging"]')?.click());
      await sleep(100);
      ok("the merge methods are the ones the repository allows", !(await panelText(page)).includes("Rebase and merge"));
      await inPanel(page, "Merge");
      await sleep(200);
      const confirm = await page.evaluate(() => [...document.querySelectorAll("h2")].find((h) => h.innerText === "Merge pull request #1?")?.parentElement?.parentElement?.innerText ?? "");
      ok("Merge asks first: what goes where, how, as whom, and only the reviewed commit", confirm.includes("feat/1 goes into main of acme/app by squashing, as @work-me") && confirm.includes("(up to abc1234)") && confirm.includes("The branch feat/1 is deleted afterwards."), confirm);
      ok("  nothing was merged before confirming", (await gcalls(page, "github_pull_merge")).length === 0);
      const listBefore = (await gcalls(page, "github_pulls")).length;
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Merge #1")?.click());
      await page.waitForFunction(() => (document.querySelector('[role=dialog][aria-label^="Pull request"]')?.innerText ?? "").includes("Merged by work-me"), { timeout: 4000 }).catch(() => null);
      const m = (await gcalls(page, "github_pull_merge"))[0] ?? {};
      ok("  confirmed: merged by squashing, with the reviewed commit and title, the branch deleted", m.method === "squash" && m.sha === "abc1234def5678" && m.title === "PR 1 (#1)" && m.deleteBranch === true, JSON.stringify(m));
      ok("  the pull request reads merged, and the list is read again", (await panelText(page)).includes("Merged by work-me") && (await gcalls(page, "github_pulls")).length > listBefore, `${(await gcalls(page, "github_pulls")).length} > ${listBefore}; ${(await panelText(page)).slice(0, 300)}`);
      ok("  a merged one offers no review or merge", (await inPanel(page, "Merge")) === "missing" && (await inPanel(page, "Comment")) === "missing");
      await page.evaluate(() => document.querySelector('button[aria-label="Close pull request"]')?.click());
      await sleep(200);
      ok("closing it goes back to the list, now without it open", !(await panelText(page)) && JSON.stringify(await pullRows(page)) === JSON.stringify(["acme/app#2"]), JSON.stringify(await pullRows(page)));
      await page.evaluate(() => document.querySelector('[data-pull-row="acme/app#2"]')?.click());
      await page.waitForFunction(() => (document.querySelector('[role=dialog][aria-label^="Pull request"]')?.innerText ?? "").includes("draft"), { timeout: 4000 }).catch(() => null);
      ok("a draft cannot be merged, and says why", (await inPanel(page, "Merge")) === "disabled" && (await panelText(page)).includes("A draft cannot be merged."));
      await page.close();

      const own = await open(SCENARIOS.clean, { gh: { ...GH, detail: { ...DETAIL, author: "work-me", mergeable: false, mergeable_state: "dirty" }, reviewRefuse: "x" } });
      await openTab(own);
      await own.evaluate(() => document.querySelector('[data-pull-row="acme/app#1"]')?.click());
      await own.waitForFunction(() => (document.querySelector('[role=dialog][aria-label^="Pull request"]')?.innerText ?? "").includes("conflicts"), { timeout: 4000 }).catch(() => null);
      ok("on your own pull request Approve and Request changes are not offered, and why", (await inPanel(own, "Approve")) === "missing" && (await inPanel(own, "Request changes")) === "missing" && (await panelText(own)).includes("only lets them comment"));
      ok("conflicts: said, and Merge waits", (await panelText(own)).includes("This branch has conflicts with main") && (await inPanel(own, "Merge")) === "disabled");
      await own.close();

      const refuse = await open(SCENARIOS.clean, { gh: { ...GH, detail: { ...DETAIL, author: "pal" }, reviewRefuse: "GitHub does not let @work-me approve their own pull request.", mergeRefuse: "New commits were pushed to this branch after you reviewed it, so nothing was merged. Refresh, look at them, and merge again." } });
      await openTab(refuse);
      await refuse.evaluate(() => document.querySelector('[data-pull-row="acme/app#1"]')?.click());
      await refuse.waitForFunction(() => (document.querySelector('[role=dialog][aria-label^="Pull request"]')?.innerText ?? "").includes("No conflicts"), { timeout: 4000 }).catch(() => null);
      await inPanel(refuse, "Approve");
      await sleep(250);
      ok("a refused review shows GitHub's reason in the panel", (await panelText(refuse)).includes("does not let @work-me approve"));
      await inPanel(refuse, "Merge");
      await sleep(150);
      await refuse.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Merge #1")?.click());
      await sleep(300);
      ok("a refused merge says why, and the pull request stays open", (await panelText(refuse)).includes("New commits were pushed to this branch after you reviewed it") && (await inPanel(refuse, "Merge")) === "clicked");
      await refuse.close();
    }

    section("Collaborators: who it is shared with, invite with a role, remove after asking, cancel an invitation; a non-admin sees why not");
    {
      const page = await open(SCENARIOS.clean, { gh: GH });
      await openTab(page);
      await pill(page, "Collaborators");
      await page.waitForFunction(() => document.querySelectorAll("[data-collaborator]").length > 0, { timeout: 4000 }).catch(() => null);
      const t = await text(page);
      ok("the collaborators with their roles, the owner marked, and the invitations waiting", t.includes("Collaborators · 2") && (await page.evaluate(() => document.querySelector('[data-collaborator="acme"]')?.innerText ?? "")).includes("owner") && (await page.evaluate(() => document.querySelector('[data-invitation="newbie"]')?.innerText ?? "")).includes("write"), t.slice(0, 800));
      ok("  the section picked is remembered", (await page.evaluate(() => localStorage.getItem("gitswitch:github.section"))) === '"people"');
      await page.type('input[aria-label="GitHub username to invite"]', "friend");
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Write")?.click());
      await sleep(100);
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Maintain")?.click());
      await sleep(100);
      await (await button(page, "Invite"))?.click();
      await sleep(300);
      ok("Invite sends the username and the role picked, then shows the invitation", JSON.stringify((await gcalls(page, "github_collaborator_add"))[0]) === JSON.stringify({ account: "gh:work-me", owner: "acme", name: "app", username: "friend", permission: "maintain" }) && !!(await page.$('[data-invitation="friend"]')), JSON.stringify(await gcalls(page, "github_collaborator_add")));
      await page.evaluate(() => document.querySelector('button[aria-label="Remove pal"]')?.click());
      await sleep(150);
      ok("Remove asks first, and nothing is removed yet", (await text(page)).includes("pal loses access to acme/app") && (await gcalls(page, "github_collaborator_remove")).length === 0);
      await (await button(page, "Remove access"))?.click();
      await sleep(300);
      ok("  confirmed: removed", JSON.stringify((await gcalls(page, "github_collaborator_remove"))[0]) === JSON.stringify({ account: "gh:work-me", owner: "acme", name: "app", username: "pal" }) && !(await page.$('[data-collaborator="pal"]')));
      await page.evaluate(() => document.querySelector('button[aria-label="Cancel the invitation for newbie"]')?.click());
      await sleep(300);
      ok("cancelling an invitation takes it away", (await gcalls(page, "github_invitation_cancel"))[0]?.id === 77 && !(await page.$('[data-invitation="newbie"]')));
      await page.close();

      const reader = await open(SCENARIOS.clean, { gh: { ...GH, context: { ...GH.context, suggested: "gh:me-home", profile_login: "me-home" } } });
      await openTab(reader);
      await pill(reader, "Collaborators");
      await sleep(400);
      ok("an account that is not an admin: inviting and removing are greyed, and it says why", (await text(reader)).includes("Only an admin of this repository can change who has access") && (await reader.$eval('input[aria-label="GitHub username to invite"]', (i) => i.disabled)));
      await reader.close();
    }

    section("Pull requests page: the account's repositories with their counts, each repository's own pull requests — whose, state, Refresh");
    {
      const REPO = (repo, open, to_review, mine, extra = {}) => ({ repo, private: true, archived: false, fork: false, pushed_at: "2026-10-30T00:00:00Z", permission: "admin", open, to_review, mine, affiliated: true, ...extra });
      const PULL_REPOS = {
        login: "me-home",
        truncated: false,
        notes: ["Some organizations' repositories are hidden: they use single sign-on."],
        repos: [REPO("acme/app", 2, 1, 1), REPO("other/lib", null, 0, 1, { affiliated: false, private: false, permission: null }), REPO("quiet/repo", 0, 0, 0, { archived: true })],
      };
      const LIB_PULL = PR(7, "open", { repo: "other/lib", title: "Lib fix", author: "me-home" });
      const G = { ...GH, pullRepos: PULL_REPOS, pulls: [...GH.pulls, LIB_PULL], search: [...GH.search, { ...LIB_PULL, head_ref: null, base_ref: null }] };
      const repoRows = (page) => page.evaluate(() => [...document.querySelectorAll("[data-pull-repo]")].map((r) => r.dataset.pullRepo));
      const repoRow = (page, r) => page.evaluate((r) => document.querySelector(`[data-pull-repo="${r}"]`)?.innerText.replace(/\s+/g, " ").trim() ?? "", r);
      const pickRepo = (page, r) => page.evaluate((r) => document.querySelector(`[data-pull-repo="${r}"]`)?.click(), r);
      const groupPill = (page, group, label) => page.evaluate((group, label) => { const b = [...document.querySelectorAll(`[role=group][aria-label="${group}"] button`)].find((x) => x.innerText.trim().startsWith(label)); b?.click(); return !!b; }, group, label);
      const toPulls = async (page) => {
        await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim() === "Pull requests")?.click());
        await page.waitForFunction(() => document.querySelectorAll("[data-pull-repo]").length > 0, { timeout: 5000 }).catch(() => null);
        await sleep(300);
      };

      const page = await open(SCENARIOS.clean, { gh: G });
      await toPulls(page);
      ok("the sidebar has a Pull requests page that lists repositories", (await text(page)).includes("Your repositories and their pull requests"));
      ok("  the account's repositories are asked once, for the first account", JSON.stringify(await gcalls(page, "github_pull_repos")) === JSON.stringify([{ account: "gh:me-home" }]), JSON.stringify(await gcalls(page, "github_pull_repos")));
      ok("  the ones with open pull requests, busiest first; one without is left out until All", JSON.stringify(await repoRows(page)) === JSON.stringify(["acme/app", "other/lib"]), JSON.stringify(await repoRows(page)));
      const app = await repoRow(page, "acme/app");
      ok("  each says how many are open, asking you for a review, and yours", app.includes("2 open") && app.includes("1 to review") && app.includes("1 yours"), app);
      ok("  someone else's repository known from a pull request of yours shows only what is known", (await repoRow(page, "other/lib")) === "other/lib 1 yours", await repoRow(page, "other/lib"));
      ok("  what was withheld is said under the list", (await text(page)).includes("Some organizations' repositories are hidden"));
      ok("the first repository is picked, and only its pull requests are asked, as the account", JSON.stringify((await gcalls(page, "github_pulls")).at(-1)) === JSON.stringify({ account: "gh:me-home", owner: "acme", name: "app", state: "open", page: 1 }) && JSON.stringify(await pullRows(page)) === JSON.stringify(["acme/app#1", "acme/app#2"]), JSON.stringify(await gcalls(page, "github_pulls")));
      ok("  its header names it and what the account may do", (await page.evaluate(() => document.querySelector('[aria-label="Pull requests of the repository"] h2')?.innerText ?? "")) === "acme/app" && (await text(page)).includes("admin"));
      await pickRepo(page, "other/lib");
      await sleep(300);
      ok("another repository: its own pull requests, and the pick is remembered for this account", JSON.stringify((await gcalls(page, "github_pulls")).at(-1)) === JSON.stringify({ account: "gh:me-home", owner: "other", name: "lib", state: "open", page: 1 }) && JSON.stringify(await pullRows(page)) === JSON.stringify(["other/lib#7"]) && (await page.evaluate(() => localStorage.getItem("gitswitch:pulls.repoByAccount") ?? "")).includes('"gh:me-home":"other/lib"'), JSON.stringify(await pullRows(page)));
      await groupPill(page, "Whose pull requests", "Review requested");
      await sleep(300);
      ok("whose: a search in this repository only, and the rows need not repeat its name", JSON.stringify((await gcalls(page, "github_pulls_search")).at(-1)) === JSON.stringify({ account: "gh:me-home", scope: "review_requested", state: "open", repo: "other/lib", page: 1 }) && (await page.evaluate(() => document.querySelector('[data-pull-row="other/lib#7"]')?.innerText ?? "")).includes("#7 by me-home") && !(await page.evaluate(() => document.querySelector('[data-pull-row="other/lib#7"]')?.innerText ?? "")).includes("other/lib #7"), JSON.stringify((await gcalls(page, "github_pulls_search")).at(-1)));
      ok("  the pick is remembered", (await page.evaluate(() => localStorage.getItem("gitswitch:pulls.whose"))) === '"review_requested"');
      await groupPill(page, "Whose pull requests", "Everyone's");
      await groupPill(page, "Pull request state", "Merged");
      await sleep(300);
      ok("state: the repository's merged ones", (await gcalls(page, "github_pulls")).at(-1)?.state === "merged" && (await gcalls(page, "github_pulls")).at(-1)?.name === "lib");
      await groupPill(page, "Pull request state", "Open");
      await groupPill(page, "Which repositories", "To review");
      await sleep(200);
      ok("To review: only the repositories asking you for a review", JSON.stringify(await repoRows(page)) === JSON.stringify(["acme/app"]), JSON.stringify(await repoRows(page)));
      await groupPill(page, "Which repositories", "All");
      await sleep(200);
      ok("All: every repository, archived ones marked", JSON.stringify(await repoRows(page)) === JSON.stringify(["acme/app", "other/lib", "quiet/repo"]) && !!(await page.$('[data-pull-repo="quiet/repo"] [aria-label="archived"]')), JSON.stringify(await repoRows(page)));
      await page.type('input[aria-label="Filter repositories"]', "quiet");
      await sleep(150);
      ok("  the filter box narrows the repositories", JSON.stringify(await repoRows(page)) === JSON.stringify(["quiet/repo"]), JSON.stringify(await repoRows(page)));
      await pickRepo(page, "quiet/repo");
      await sleep(300);
      ok("  a repository without open pull requests says so", (await text(page)).includes("No open pull requests."));
      await page.click('input[aria-label="Filter repositories"]', { clickCount: 3 });
      await page.keyboard.press("Backspace");
      await pickRepo(page, "acme/app");
      await sleep(300);
      const marks = { repos: (await gcalls(page, "github_pull_repos")).length, accounts: (await gcalls(page, "repo_accounts")).length, pulls: (await gcalls(page, "github_pulls")).length };
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Refresh")?.click());
      await sleep(400);
      ok("one Refresh reads the accounts, the repositories and the list again", (await gcalls(page, "github_pull_repos")).length === marks.repos + 1 && (await gcalls(page, "repo_accounts")).length === marks.accounts + 1 && (await gcalls(page, "github_pulls")).length === marks.pulls + 1, JSON.stringify(marks));
      ok("  and the list has no Refresh of its own here", (await page.evaluate(() => [...document.querySelectorAll("button")].filter((b) => b.innerText.trim() === "Refresh").length)) === 1);
      await page.evaluate(() => document.querySelector('[data-pull-row="acme/app#1"]')?.click());
      await sleep(400);
      ok("a pull request opens to review and merge, as the account picked", JSON.stringify((await gcalls(page, "github_pull")).at(-1)) === JSON.stringify({ account: "gh:me-home", owner: "acme", name: "app", number: 1 }) && (await panelText(page)).includes("me-home wants to merge"));
      await page.close();

      const gone = await open(SCENARIOS.clean, { gh: G, storage: { "pulls.repoByAccount": { "gh:me-home": "gone/repo" }, "pulls.whose": "nonsense", "pulls.repoFilter": "open" } });
      await toPulls(gone);
      ok("a remembered repository that is gone: the first one is shown, and the gone one is never asked for", JSON.stringify((await gcalls(gone, "github_pulls")).map((c) => `${c.owner}/${c.name}`)) === JSON.stringify(["acme/app"]), JSON.stringify(await gcalls(gone, "github_pulls")));
      ok("  an unknown remembered 'whose' falls back to everyone's", (await gcalls(gone, "github_pulls_search")).length === 0);
      await gone.close();

      const none = await open(SCENARIOS.clean, { gh: { ...G, pullRepos: { ...PULL_REPOS, notes: [], repos: [REPO("quiet/repo", 0, 0, 0)] } } });
      await none.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim() === "Pull requests")?.click());
      await none.waitForFunction(() => document.body.innerText.includes("has an open pull request"), { timeout: 5000 }).catch(() => null);
      ok("no repository with open pull requests: said, nothing asked, and All is offered", (await text(none)).includes("None of @me-home's 1 repositories has an open pull request.") && (await gcalls(none, "github_pulls")).length === 0 && !!(await button(none, "Show all repositories")));
      await (await button(none, "Show all repositories"))?.click();
      await sleep(300);
      ok("  Show all lists it and shows its pull requests", JSON.stringify(await repoRows(none)) === JSON.stringify(["quiet/repo"]) && (await gcalls(none, "github_pulls")).at(-1)?.name === "repo");
      await none.close();
    }
  }

  section("Uninstall tab: the apps, one uninstalled step by step, then only its leftovers — ids sent, never paths");
  {
    const calls = (page, cmd) => page.evaluate((c) => window.__CALLS__.filter((x) => x.cmd === c).map((x) => x.args), cmd);
    const rows = (page) => page.evaluate(() => [...document.querySelectorAll("[data-app-row]")].map((r) => r.getAttribute("data-app-row")));
    const page = await openUninstallPage(browser, server);
    await page.waitForFunction(() => document.querySelectorAll("[data-app-row]").length === 4, { timeout: 5000 }).catch(() => null);
    ok("the apps are listed by name", JSON.stringify(await rows(page)) === JSON.stringify(["Bar Tool", "Foo Writer", "GitSwitch", "Xcode"]), JSON.stringify(await rows(page)));
    await page.evaluate(() => [...document.querySelectorAll('[aria-label="Sort"] button')].find((b) => b.innerText === "Size").click());
    await sleep(200);
    ok("  by size, the biggest first", (await rows(page)).slice(0, 2).join() === "Bar Tool,Foo Writer");
    await page.type('input[aria-label="Find an app"]', "foo");
    await sleep(200);
    ok("  the search narrows them", JSON.stringify(await rows(page)) === JSON.stringify(["Foo Writer"]));
    await page.evaluate(() => { const i = document.querySelector('input[aria-label="Find an app"]'); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, ""); i.dispatchEvent(new Event("input", { bubbles: true })); });
    await sleep(200);
    ok("one GitSwitch can't remove says why, and its button is off", await page.evaluate(() => { const r = document.querySelector('[data-app-row="Xcode"]'); return r.innerText.includes("Not removable here: GitSwitch's git comes from it") && r.querySelector("[data-app-uninstall]").disabled; }));
    await page.evaluate(() => document.querySelector('[data-app-row="GitSwitch"] [data-app-uninstall]').click());
    await sleep(300);
    ok("GitSwitch's own row opens its own uninstall", (await calls(page, "uninstall_plan")).length === 1 && (await calls(page, "apps_prepare")).length === 0);
    await page.keyboard.press("Escape");
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Cancel")?.click());
    await sleep(200);

    await page.evaluate(() => document.querySelector('[data-app-row="Foo Writer"] [data-app-uninstall]').click());
    await page.waitForFunction(() => document.querySelector('[data-flow-stage="confirm"]'), { timeout: 5000 }).catch(() => null);
    const prep = await calls(page, "apps_prepare");
    ok("the confirm step asks for the app by its id only", prep.length === 1 && JSON.stringify(prep[0]) === JSON.stringify({ id: "mac:/Applications/Foo Writer.app" }), JSON.stringify(prep));
    ok("  and says what will happen", (await text(page)).includes("/Applications/Foo Writer.app is moved to the Trash."));
    await page.evaluate(() => document.querySelector("[data-flow-go]").click());
    await page.waitForFunction(() => document.querySelector('[data-flow-stage="gone"]'), { timeout: 5000 }).catch(() => null);
    ok("uninstalled with the session, nothing else", JSON.stringify(await calls(page, "apps_uninstall")) === JSON.stringify([{ session: "sess-FooWriter" }]));
    ok("  no registry choice on a Mac", !(await page.$("[data-flow-registry]")));
    await page.evaluate(() => document.querySelector("[data-flow-scan]").click());
    await page.waitForFunction(() => document.querySelector('[data-flow-stage="leftovers"]'), { timeout: 5000 }).catch(() => null);
    ok("its leftovers are asked for with the session", JSON.stringify(await calls(page, "apps_scan")) === JSON.stringify([{ session: "sess-FooWriter", registry: false }]));
    const ticks = await page.evaluate(() => [...document.querySelectorAll("[data-leftover]")].map((l) => [l.getAttribute("data-leftover"), l.querySelector('input[type="checkbox"]').checked]));
    ok("the sure ones are ticked, the maybe is not", JSON.stringify(ticks) === JSON.stringify([["l2", false], ["l0", true], ["l1", true]]) || JSON.stringify(ticks.sort()) === JSON.stringify([["l0", true], ["l1", true], ["l2", false]]), JSON.stringify(ticks));
    ok("the administrator's one has a command and no checkbox", await page.evaluate(() => { const a = document.querySelector('[data-leftover-admin="l3"]'); return !!a && !a.querySelector('input[type="checkbox"]') && a.innerText.includes("sudo rm -rf"); }));
    ok("  the notes are shown", (await text(page)).includes("sudo pkgutil --forget com.acme.pkg"));
    ok("the button counts what is ticked", (await text(page)).includes("Move 2 to the Trash"));
    await page.evaluate(() => document.querySelector('[data-leftover="l2"] input[type="checkbox"]').click());
    await sleep(150);
    ok("  ticking the maybe counts it", (await text(page)).includes("Move 3 to the Trash"));
    await page.evaluate(() => document.querySelector("[data-flow-delete]").click());
    await page.waitForFunction(() => document.querySelector('[data-flow-stage="done"]'), { timeout: 5000 }).catch(() => null);
    const del = await calls(page, "apps_delete");
    ok("deleting sends the scan and the ticked ids — never a path", del.length === 1 && del[0].scan === "scan-1" && JSON.stringify([...del[0].ids].sort()) === JSON.stringify(["l0", "l1", "l2"]) && !JSON.stringify(del).includes("/"), JSON.stringify(del));
    ok("  and says what happened to each", (await page.$$('[data-result="trashed"]')).length === 3);
    await page.close();

    const run = await openUninstallPage(browser, server, { prepared: { "mac:/Applications/Foo Writer.app": { running: true } } });
    await run.evaluate(() => document.querySelector('[data-app-row="Foo Writer"] [data-app-uninstall]').click());
    await run.waitForFunction(() => document.querySelector('[data-flow-stage="confirm"]'), { timeout: 5000 }).catch(() => null);
    ok("a running app: quit it first, the button is off", (await text(run)).includes("Foo Writer is running. Quit it first.") && (await run.evaluate(() => document.querySelector("[data-flow-go]").disabled)));
    await run.close();

    const still = await openUninstallPage(browser, server, { uninstall: { state: "still-installed", message: "Another copy with the same id is installed at /Users/me/Applications/Foo Writer.app — its files are its own." } });
    await still.evaluate(() => document.querySelector('[data-app-row="Foo Writer"] [data-app-uninstall]').click());
    await still.waitForFunction(() => document.querySelector('[data-flow-stage="confirm"]'), { timeout: 5000 }).catch(() => null);
    await still.evaluate(() => document.querySelector("[data-flow-go]").click());
    await still.waitForFunction(() => document.querySelector('[data-flow-stage="still"]'), { timeout: 5000 }).catch(() => null);
    ok("still installed (another copy): said, and no leftovers are looked for", (await text(still)).includes("Another copy with the same id") && (await calls(still, "apps_scan")).length === 0);
    await still.close();

    const resumed = await openUninstallPage(browser, server, { sessions: [{ session: "sess-old", name: "Old App", state: "gone", started: 1760000000 }] });
    ok("an uninstall not finished (a restart in between) is offered", (await text(resumed)).includes("Old App") && (await text(resumed)).includes("its leftovers were not looked at"));
    await (await button(resumed, "Continue")).click();
    await resumed.waitForFunction(() => document.querySelector('[data-flow-stage="gone"]'), { timeout: 5000 }).catch(() => null);
    await resumed.evaluate(() => document.querySelector("[data-flow-scan]").click());
    await sleep(300);
    ok("  Continue looks for that one's leftovers", JSON.stringify(await calls(resumed, "apps_scan")) === JSON.stringify([{ session: "sess-old", registry: false }]));
    await resumed.close();
  }

  section("Uninstall tab on Windows and Ubuntu: the uninstaller followed, the registry only when asked, apt's extra packages confirmed");
  {
    const calls = (page, cmd) => page.evaluate((c) => window.__CALLS__.filter((x) => x.cmd === c).map((x) => x.args), cmd);
    const win = await openUninstallPage(browser, server, {
      platform: "windows",
      apps: [{ id: "win:HKLM64:7-Zip", name: "7-Zip 23.01 (x64)", version: "23.01", publisher: "Igor Pavlov", location: "C:\\Program Files\\7-Zip", source: "exe", size: 5800000, date: null, notes: [], blocked: null, is_self: false }],
      prepared: { "win:HKLM64:7-Zip": { method: "Its own uninstaller opens; follow its steps.", command: '"C:\\Program Files\\7-Zip\\Uninstall.exe"' } },
      uninstall: { state: "uninstalling", message: "The uninstaller is open — follow its steps." },
      status: [{ state: "uninstalling", message: "The uninstaller is open — follow its steps." }, { state: "gone", message: null }],
    });
    await win.evaluate(() => document.querySelector("[data-app-uninstall]").click());
    await win.waitForFunction(() => document.querySelector('[data-flow-stage="confirm"]'), { timeout: 5000 }).catch(() => null);
    ok("the exact command that runs is shown", (await text(win)).includes('"C:\\Program Files\\7-Zip\\Uninstall.exe"'));
    await win.evaluate(() => document.querySelector("[data-flow-go]").click());
    await win.waitForFunction(() => document.querySelector('[data-flow-stage="running"]'), { timeout: 5000 }).catch(() => null);
    ok("while the uninstaller runs, it is followed, and \"It's finished\" is offered", (await text(win)).includes("follow its steps") && !!(await button(win, "It's finished — check")));
    await win.waitForFunction(() => document.querySelector('[data-flow-stage="gone"]'), { timeout: 8000 }).catch(() => null);
    ok("  it moves on when the uninstaller is done", !!(await win.$('[data-flow-stage="gone"]')) && (await calls(win, "apps_status")).length >= 2);
    ok("the registry is a choice, off by default", await win.evaluate(() => { const r = document.querySelector("[data-flow-registry] input"); return !!r && !r.checked; }));
    await win.evaluate(() => document.querySelector("[data-flow-registry] input").click());
    await win.evaluate(() => document.querySelector("[data-flow-scan]").click());
    await sleep(300);
    ok("  ticked, the scan includes it", JSON.stringify(await calls(win, "apps_scan")) === JSON.stringify([{ session: "sess-7Zip2301x64", registry: true }]));
    ok("  and Windows' word is used", (await text(win)).includes("to the Recycle Bin"));
    await win.close();

    const deb = { id: "deb:xterm", name: "XTerm", version: "379", publisher: null, location: "package xterm", source: "deb", size: 1400000, date: null, notes: [], blocked: null, is_self: false };
    const lin = await openUninstallPage(browser, server, {
      platform: "linux",
      apps: [deb],
      packages: [{ ...deb, id: "pkg:libfoo1", name: "libfoo1", source: "package" }],
      prepared: { "deb:xterm": { method: "The package xterm and the 1 other package(s) listed are removed; the system asks for your password.", command: "pkexec env DEBIAN_FRONTEND=noninteractive dpkg -r xterm xterm-extra", also_removed: ["xterm-extra"] } },
    });
    ok("Ubuntu shows Apps and All packages", (await text(lin)).includes("All packages"));
    await lin.evaluate(() => document.querySelector("[data-app-uninstall]").click());
    await lin.waitForFunction(() => document.querySelector('[data-flow-stage="confirm"]'), { timeout: 5000 }).catch(() => null);
    ok("what apt would also remove is shown, and Uninstall waits for a yes", (await text(lin)).includes("xterm-extra") && (await lin.evaluate(() => document.querySelector("[data-flow-go]").disabled)));
    await lin.evaluate(() => document.querySelector("[data-flow-also] input").click());
    await sleep(150);
    ok("  ticked \"Remove them too\": it can go", !(await lin.evaluate(() => document.querySelector("[data-flow-go]").disabled)));
    await lin.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Cancel").click());
    await sleep(200);
    await lin.evaluate(() => [...document.querySelectorAll('[aria-label="What to list"] button')].find((b) => b.innerText === "All packages").click());
    await sleep(300);
    ok("All packages asks for every package", (await calls(lin, "apps_list")).some((a) => a.kind === "packages") && (await text(lin)).includes("libfoo1"));
    ok("  with a warning about what packages are", (await text(lin)).includes("the system's own packages can't be removed here"));
    await lin.close();
  }

  section("Settings › Uninstall GitSwitch: what is taken back and what is kept, the locks refused stops everything");
  {
    const calls = (page, cmd) => page.evaluate((c) => window.__CALLS__.filter((x) => x.cmd === c).map((x) => x.args), cmd);
    const plan = { platform: "macos", app: { kind: "trash", bundle: "/Applications/GitSwitch.app", trash: "/Users/me/.Trash" }, how: "/Applications/GitSwitch.app is moved to the Trash once GitSwitch has quit.", gitconfig: "/Users/me/.gitconfig", signers: null, repos: ["/w/one", "/w/two"], lock_helper: true, locks: ["/w/two"], signins: ["me-home"], data: ["/Users/me/Library/Application Support/com.gitswitch.app"] };
    const page = await openUninstallPage(browser, server, { path: "/settings", plan });
    await page.evaluate(() => document.querySelector("[data-uninstall-open]").click());
    await page.waitForFunction(() => document.querySelector("[data-uninstall-now]"), { timeout: 5000 }).catch(() => null);
    const t = await text(page);
    ok("the dialog lists what is taken back now", t.includes("The push locks (1) and the lock helper") && t.includes("in 2 repositories") && t.includes("/Users/me/.gitconfig") && t.includes("@me-home"));
    ok("  what goes once GitSwitch has quit", t.includes("com.gitswitch.app") && t.includes("moved to the Trash once GitSwitch has quit"));
    ok("  and what is kept", t.includes("Your SSH keys") && t.includes("Your repositories and their files"));
    await page.evaluate(() => document.querySelector('[aria-label="Keep my profiles and sign-ins"]').click());
    await sleep(150);
    const kept = await text(page);
    ok("keeping data moves the sign-ins and data to Kept", !kept.includes("@me-home") && !kept.includes("com.gitswitch.app") && kept.includes("Your profiles, settings and GitHub sign-ins"));
    await page.evaluate(() => document.querySelector("[data-uninstall-go]").click());
    await page.waitForFunction(() => document.querySelector("[data-uninstall-closing]"), { timeout: 5000 }).catch(() => null);
    ok("it takes back, then removes — both told to keep the data", JSON.stringify(await calls(page, "uninstall_prepare")) === JSON.stringify([{ keepData: true }]) && JSON.stringify(await calls(page, "uninstall_finish")) === JSON.stringify([{ keepData: true }]));
    ok("  and says what happens next", (await text(page)).includes("moved to the Trash once GitSwitch has quit"));
    await page.close();

    const refused = await openUninstallPage(browser, server, { path: "/settings", plan, selfPrepare: { outcome: "stopped", message: "The push locks were not removed (cancelled), so nothing was uninstalled.", command: "sudo /x/helper --job /x/job.json", done: [], problems: [] } });
    await refused.evaluate(() => document.querySelector("[data-uninstall-open]").click());
    await refused.waitForFunction(() => document.querySelector("[data-uninstall-go]"), { timeout: 5000 }).catch(() => null);
    await refused.evaluate(() => document.querySelector("[data-uninstall-go]").click());
    await refused.waitForFunction(() => document.querySelector("[data-uninstall-stopped]"), { timeout: 5000 }).catch(() => null);
    ok("the locks refused: stopped, with the command, and nothing removed", (await text(refused)).includes("nothing was uninstalled") && (await text(refused)).includes("sudo /x/helper") && (await calls(refused, "uninstall_finish")).length === 0);
    await refused.close();
  }
} finally {
  await browser.close();
  server.close();
}

process.exit(t.done() ? 0 : 1);
