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
  BRANCH_DELETE_OUTCOME, sub, PULL_MOVED_OUTCOME, CONFLICT_VIEW } from "./fixtures.mjs";

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
    treeReset: TREE_RESET_OUTCOME,
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
            case "terminal_open":
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
              return Promise.resolve(args.path === "logo.png" ? diffBinary : diffText);
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
            case "changes_create_branch":
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
  const cfg = { firstDelayBob: opts.firstDelayBob ?? 700, restDelay: opts.restDelay ?? 400 };
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
            return { clones: {}, owner_profiles: {}, org_ssh_users: {} };
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
            return Promise.resolve([]);
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
    ok("uninitialised ones are grouped, not listed 27 times", s.includes("27 not downloaded yet"));
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
    const page = await open(SCENARIOS.submodules, { submodules: [detached], subInner: SUB_INNER });
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
    ok("  so it offers Record this pointer instead", trinity?.includes("Record this pointer") === true, JSON.stringify(trinity));
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
    ok("a conflicted gitlink row has no side buttons (the sync finishes it)", row !== null && !row.buttons.some((b) => b.title.startsWith("Keep mine") || b.title.startsWith("Take theirs")));
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
} finally {
  await browser.close();
  server.close();
}

process.exit(t.done() ? 0 : 1);
