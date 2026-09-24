// Headless checks for the Changes page against a mocked backend.
// Serves the built app and replaces Tauri's invoke bridge before any app code
// runs, so every repo state can be shown without touching a real repository.
import { launch, serve, tally, DIST_OK } from "./browser.mjs";
import {
  REPOS,
  SCENARIOS,
  DIFF_TEXT,
  DIFF_BINARY,
  SUBMODULES,
  SUB_INNER,
  LFS_POINTERS,
  LFS_PRESENT,
  LFS_NOT_INSTALLED,
  SYNC_PLAN,
  SYNC_PLAN_NOSTASH,
  SYNC_PLAN_BLOCKED,
  SYNC_PLAN_NOTHING,
  SYNC_OUTCOME,
  SYNC_OUTCOME_UNRECORDED,
  BRANCHES,
  STASHES,
  STASH_DETAIL,
  SUB_STATUSES,
  SUB_QUIET,
  SUBMODULE_A,
  TREE_RESET_OUTCOME,
  STASH_DROP_OUTCOME,
  BRANCH_DELETE_OUTCOME,
} from "./fixtures.mjs";

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
    lockOutcome: opts.lockOutcome ?? "applied",
    syncPlan: opts.syncPlan ?? SYNC_PLAN,
    syncPlanNoStash: opts.syncPlanNoStash ?? SYNC_PLAN_NOSTASH,
    syncOutcome: opts.syncOutcome ?? SYNC_OUTCOME,
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
  };
  const page = await browser.newPage();
  await page.setViewport({ width: cfg.width, height: 900 });
  await page.evaluateOnNewDocument(
    (repos, status, diffText, diffBinary, cfg) => {
      window.__CALLS__ = [];
      window.__SCENARIO__ = status;
      localStorage.clear();
      localStorage.setItem("gitswitch:changes.repo", JSON.stringify("/repos/gitswitch"));
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
        invoke: (cmd, args) => {
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
              return Promise.resolve(repos);
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
            case "changes_file_diff":
              return Promise.resolve(args.path === "logo.png" ? diffBinary : diffText);
            case "changes_pull":
            case "changes_push":
            case "changes_submodule_update":
            case "changes_lfs_pull":
              if (cfg.hang) return new Promise(() => {}); // never settles
              return done("Done.");
            case "changes_sync_plan":
              return Promise.resolve(args.stash === false ? cfg.syncPlanNoStash : cfg.syncPlan);
            case "changes_sync_run":
              if (cfg.hang) return new Promise(() => {}); // never settles
              return Promise.resolve({
                ok: true,
                headline: "Synced: 2 commits of yours on top of 3 new commits from origin/main.",
                detail: "Your uncommitted changes were stashed and re-applied.",
                status: window.__SCENARIO__,
                sync: cfg.syncOutcome,
              });
            case "changes_sync_continue":
              return done("Synced: 3 commits of yours on top of 3 new commits from origin/main.");
            case "changes_sync_abort":
              return done("Sync aborted.");
            case "changes_record_pointers":
              return done("Recorded 1 submodule pointer(s).");
            case "changes_stage":
            case "changes_unstage":
            case "changes_discard":
            case "changes_abort":
            case "changes_continue":
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
            case "changes_push_state":
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
    },
    REPOS,
    scenario,
    DIFF_TEXT,
    DIFF_BINARY,
    cfg
  );
  await page.goto(`${server.url}/changes`, { waitUntil: "networkidle0" });
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

/** The Clone page with its own mock bridge: it asks different questions than Changes. */
async function openClonePage(browser, server, opts = {}) {
  const cfg = {
    lfsTool: opts.lfsTool ?? { installed: true, version: "git-lfs/3.5.1 (GitHub; darwin arm64)", install_hint: "brew install git-lfs" },
    cloneResult: opts.cloneResult ?? null,
    mode: opts.mode ?? "full",
  };
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 900 });
  await page.evaluateOnNewDocument((cfg) => {
    window.__CALLS__ = [];
    localStorage.clear();
    localStorage.setItem("gitswitch:sparse.url", JSON.stringify("git@github.com:me/big-data.git"));
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
            return Promise.resolve(cfg.cloneResult);
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
  section("Sync: the latest from upstream, your commits on top");
  const stashBox = (page) =>
    page.evaluate(() => document.querySelector('input[aria-label="Stash uncommitted changes during the sync"]')?.click() ?? false);
  {
    const page = await open(SCENARIOS.diverged);
    const s0 = await text(page);
    ok("the card explains itself before anything runs", s0.includes("your commits on top") && s0.includes("Nothing is pushed"));
    const assess = await button(page, "Assess");
    ok("Assess says it fetches", assess !== null && (await assess.evaluate((b) => b.innerText)).includes("fetches"));
    ok("Sync now is not offered before a plan exists", (await button(page, "Sync now")) === null);
    await assess.click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    const s1 = await text(page);
    ok("the plan is a sentence", s1.includes("Your 2 commits are replayed on top of 3 new commits from origin/main"));
    ok("  the submodule table names A's rebase and its base", s1.includes("rebase your commits onto 7a9d38a"));
    ok("  and B's fast-forward", s1.includes("fast-forward to 3c3c3c3"));
    ok("  a skipped gitlink is collapsed, not hidden", s1.includes("1 gitlink left alone"));
    ok("  the stash choice shows the count", s1.includes("Stash my 1 uncommitted change during the sync"));
    ok("  file overlap is a warning, not a blocker", s1.includes("Both sides changed shared.txt") && (await button(page, "Sync now")) !== null);
    ok("  Sync now is enabled", !(await (await button(page, "Sync now")).evaluate((b) => b.disabled)));
    const calls0 = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_sync_plan"));
    ok("  Assess fetched, with stashing on", calls0.length === 1 && calls0[0].args.fetch === true && calls0[0].args.stash === true);
    await stashBox(page);
    await page.waitForFunction(() => document.body.innerText.includes("Turn on stashing"), { timeout: 4000 });
    const calls1 = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_sync_plan"));
    ok("turning stashing off re-plans without a second fetch", calls1.length === 2 && calls1[1].args.fetch === false && calls1[1].args.stash === false);
    ok("  and the dirty tree becomes a named blocker", (await text(page)).includes("Turn on stashing"));
    ok("  that disables Sync now", await (await button(page, "Sync now")).evaluate((b) => b.disabled));
    await stashBox(page);
    await page.waitForFunction(() => !document.body.innerText.includes("Turn on stashing"), { timeout: 4000 });
    await (await button(page, "Sync now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Synced:"), { timeout: 4000 });
    const s2 = await text(page);
    const run = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_sync_run"));
    ok("Sync now sends the plan's fingerprint and the choices", run && run.args.stash === true && run.args.bundles === false && run.args.fingerprint.length === 3);
    ok("the result is a table per repository", s2.includes("this repository") && s2.includes("aaaa111 → cccc333") && s2.includes("rebased onto 7a9d38a"));
    ok("  every check passed, said so", s2.includes("All 6 checks passed"));
    ok("  the backups are one click away, with the undo commands", s2.includes("Backups: 2 branches"));
    await page.evaluate(() => [...document.querySelectorAll("summary")].find((x) => x.innerText.includes("Backups"))?.click());
    await sleep(200);
    ok("  the undo command is exact", (await text(page)).includes("git -C /repos/gitswitch reset --hard gitswitch-before-sync-"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { syncPlan: SYNC_PLAN_BLOCKED });
    await (await button(page, "Assess")).click();
    await page.waitForFunction(() => document.body.innerText.includes("checked out at"), { timeout: 4000 });
    const s = await text(page);
    ok("a blocker is a sentence with the remedy", s.includes("commit that pointer (Record submodule pointers)"));
    const b = await button(page, "Sync now");
    ok("  and Sync now stays disabled", await b.evaluate((x) => x.disabled));
    ok("  saying why", (await b.evaluate((x) => x.title)).includes("blockers"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { syncPlan: SYNC_PLAN_NOTHING });
    await (await button(page, "Assess")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Nothing to sync"), { timeout: 4000 });
    ok("nothing to do is said in words, with no Sync now", (await button(page, "Sync now")) === null);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.unpublished);
    const b = await button(page, "Assess");
    ok("without an upstream Assess is disabled", await b.evaluate((x) => x.disabled));
    ok("  and the card says what to do", (await text(page)).includes("no upstream to sync from"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { hang: true });
    await (await button(page, "Assess")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Sync now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Syncing"), { timeout: 4000 });
    const s = await text(page);
    ok("a running sync is a job banner that survives leaving the page", s.includes("Syncing — fetching, rebasing") && s.includes("keeps running"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.diverged, { syncOutcome: SYNC_OUTCOME_UNRECORDED });
    await (await button(page, "Assess")).click();
    await page.waitForFunction(() => document.body.innerText.includes("replayed on top"), { timeout: 4000 });
    await (await button(page, "Sync now")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Synced:"), { timeout: 4000 });
    ok("a submodule ahead of the recorded pointer is named", (await text(page)).includes("B is checked out ahead of what this branch records"));
    const rec = await button(page, "Record submodule pointers");
    ok("  with a button to record it", rec !== null);
    await rec.click();
    await sleep(300);
    ok("  which makes the commit", (await text(page)).includes("Recorded 1 submodule pointer(s)."));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPaused);
    const s = await text(page);
    ok("a paused sync replaces the plain rebase banner", s.includes("Sync paused") && !s.includes("Rebase in progress"));
    ok("  with the hint", s.includes("Resolve the conflicts listed on this page"));
    ok("  and the file", s.includes("shared.txt"));
    ok("  the Sync card itself is gone", (await button(page, "Assess")) === null);
    const c = await button(page, "Continue sync");
    ok("  Continue sync is disabled while conflicts remain", await c.evaluate((b) => b.disabled));
    ok("  saying why", (await c.evaluate((b) => b.title)).includes("Resolve and stage"));
    ok("  the conflict list is there to resolve them", s.includes("Conflicts") && s.includes("both modified"));
    await (await button(page, "Abort sync")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Sync aborted."), { timeout: 4000 });
    ok("  Abort sync reports itself", true);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPausedInSub);
    const s = await text(page);
    ok("paused inside a submodule says so", s.includes("Sync paused inside A"));
    const c = await button(page, "Continue sync");
    ok("  Continue sync stays enabled (the gitlink conflict is the sync's to finish)", !(await c.evaluate((b) => b.disabled)));
    await c.click();
    await page.waitForFunction(() => document.body.innerText.includes("Synced: 3 commits"), { timeout: 4000 });
    ok("  and continuing reports the outcome", true);
    await page.close();
  }
  {
    const page = await open(SCENARIOS.syncPausedChild);
    const s = await text(page);
    ok("the submodule's own page knows the parent's sync is paused here", s.includes("A sync of gitswitch is paused inside this submodule"));
    ok("  and offers to go back to the parent", (await button(page, "Open gitswitch")) !== null);
    ok("  without a Continue sync of its own", (await button(page, "Continue sync")) === null);
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
    await page.evaluate(() => [...document.querySelectorAll("a")].find((a) => a.innerText.trim() === "Changes")?.click());
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
    ok("uninitialised ones are grouped, not listed 27 times", s.includes("27 not initialised"));
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
  section("Git LFS");
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    const s = await text(page);
    ok("a repo with pointer stubs gets an LFS card", s.includes("Git LFS"));
    ok("it counts the stubs in words", s.includes("5 of 5 LFS files are still a pointer stub"));
    ok("the button names how many it will fetch", s.includes("Pull LFS files (5)"));
    ok("and is enabled", !(await (await button(page, "Pull LFS files")).evaluate((b) => b.disabled)));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.startsWith("Which files"))?.click());
    await sleep(200);
    ok("the stub files can be listed", (await text(page)).includes("touchstones/axios__axios_dataset.jsonl"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_PRESENT });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    const s = await text(page);
    ok("when everything is present it says so", s.includes("All 5 LFS files are present"));
    ok("and the button is disabled", await (await button(page, "Pull LFS files")).evaluate((b) => b.disabled));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_NOT_INSTALLED });
    await page.waitForFunction(() => document.body.innerText.includes("Git LFS"), { timeout: 4000 });
    const s = await text(page);
    ok("a missing git-lfs is explained", s.includes("isn't installed"));
    ok("with the install command", s.includes("brew install git-lfs"));
    ok("and the button is disabled", await (await button(page, "Pull LFS files")).evaluate((b) => b.disabled));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.lfsRepo, { lfs: LFS_POINTERS, hang: true });
    await page.waitForFunction(() => document.body.innerText.includes("Pull LFS files (5)"), { timeout: 4000 });
    await (await button(page, "Pull LFS files")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Downloading LFS files"), { timeout: 4000 });
    ok("pulling runs as a background job with a banner", (await text(page)).includes("Downloading LFS files"));
    const call = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_lfs_pull"));
    ok("and calls the backend for the selected repo", call?.args?.repoPath === "/repos/gitswitch");
    await page.close();
  }

  // -----------------------------------------------------------------
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
    ok("  the sync card and the commit box both offer to choose a branch", offers === 2);
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
    await clickInRow(page, "fix header", { title: "Apply this stash and remove it" });
    await sleep(300);
    const pop = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "changes_stash_apply")[1]);
    ok("Pop applies and removes", pop?.args?.index === 0 && pop?.args?.pop === true);
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
    await clickInRow(page, "fix header", { title: "Restore only this file from the stash" });
    await sleep(300);
    const rf = await page.evaluate(() => window.__CALLS__.find((c) => c.cmd === "changes_stash_restore_file"));
    ok("  Restore file sends the index and that path only", rf?.args?.index === 0 && rf?.args?.path === "src/header.ts");
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
    ok("  and the tooltip points at History", (b?.title ?? "").includes("History") && (b?.title ?? "").includes("origin/main"));
    await page.close();
  }
  {
    const page = await open(SCENARIOS.clean, { refuse: { changes_undo_commit: { code: "already-pushed", message: "The last commit is already on origin/main. Revert it instead." } } });
    await (await button(page, "Undo last commit")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Open in History"), { timeout: 4000 });
    ok("a refusal because it was pushed offers to open History", (await text(page)).includes("already on origin/main"));
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
    ok("Discard everything asks first, with the counts", s.includes("(2 staged, 3 not staged, 1 new)") && s.includes("cannot be undone"));
    ok("  new files are kept unless asked", await page.$eval('input[aria-label="Also delete new files"]', (el) => !el.checked));
    ok("  a stash is offered instead", (await page.$('input[aria-label="Stash them first"]')) !== null);
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
