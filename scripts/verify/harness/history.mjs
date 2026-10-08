// Headless checks for the History page's commit actions against a mocked backend.
// Serves the built app and replaces Tauri's invoke bridge before any app code
// runs, so every reset/revert/cherry-pick can be exercised without a real repo.
import { launch, serve, tally, DIST_OK } from "./browser.mjs";
// The Changes tab is mounted beside History now and renders every submodule row in full.
import { sub } from "./fixtures.mjs";
import {
  REPO,
  REPOS,
  C2,
  C3,
  P1,
  P2,
  HISTORY_PAGE,
  BRANCHES,
  BRANCHES_DETACHED,
  SYNC_STATUS,
  COMMIT_DETAIL,
  COMMIT_DETAIL_MERGE,
  PARENTS,
  RESOLVED,
  RESOLVED_PUSHED,
  RESOLVED_NOT_CONTAINED,
  RESOLVED_MERGE,
  treeOutcome,
  DIFF_TEXT,
  STATUS_CLEAN,
  STATUS_DIRTY,
  STATUS_DETACHED,
  AUTHORS, HISTORY_PAGE_BUMP } from "./history-fixtures.mjs";

if (!DIST_OK) {
  console.error("dist/ is missing — run `bun run build` first");
  process.exit(1);
}

const t = tally();
const { ok, section } = t;

const SHORT = COMMIT_DETAIL.short;

/** Install the mock bridge and open the History page in a given repo state. */
async function openHistoryPage(browser, server, opts = {}) {
  const cfg = {
    width: opts.width ?? 1280,
    status: opts.status ?? STATUS_DIRTY,
    branches: opts.branches ?? BRANCHES,
    resolved: opts.resolved ?? RESOLVED,
    resolvedMerge: opts.resolvedMerge ?? RESOLVED_MERGE,
    refuse: opts.refuse ?? {},
    hang: opts.hang ?? false,
    submodules: opts.submodules ?? [],
    historyPage: opts.historyPage ?? null,
    /** What every commit's detail is, when a test needs another than the fixture's. */
    detail: opts.detail ?? null,
    /** The selected account's profile id, when a test needs "this account". */
    account: opts.account ?? null,
    /** Persisted settings to seed before the page loads: { "history.hidePointerOnly": true }. */
    persist: opts.persist ?? null,
    /** Milliseconds the list of repositories (and with it the accounts) takes to arrive. */
    reposDelay: opts.reposDelay ?? 0,
    /** The people who commit here, when a test needs others than the fixture's. */
    authors: opts.authors ?? null,
    /** The repositories (and with them the accounts), when a test needs others than the fixture's. */
    repos: opts.repos ?? null,
    /** changes_repo_status answers with the path it was asked about, as the backend does. */
    statusEchoesPath: opts.statusEchoesPath ?? false,
    /** changes_submodules per repository: { "/repos/gitswitch": [...], "/repos/gitswitch/trinity": [...] }. */
    submodulesByPath: opts.submodulesByPath ?? null,
    /** What a clone made from the Clone page returns, for a hand-off test. */
    cloneResult: opts.cloneResult ?? null,
    /** The Repositories page's listing: { repos, clones, pathExists }. */
    remote: opts.remote ?? null,
    peek: opts.peek ?? { branch: "main", upstream: "origin/main", remote: "origin", remote_tip: "440d0af1234567890abcdef1234567890abcdef12", known_tip: "e81d1c51234567890abcdef1234567890abcdef12", changed: true, branch_gone: false, new_commits: 3, counted_by: "github", message: "3 new commits on origin/main since your last fetch (440d0af). Nothing was downloaded — Fetch to see them." },
  };
  const page = await browser.newPage();
  await page.setViewport({ width: cfg.width, height: 900 });
  // A render crash leaves a blank page; keep its errors for the failure message.
  const pageErrors = [];
  page.on("pageerror", (e) => pageErrors.push(String(e?.stack ?? e?.message ?? e).slice(0, 600)));
  page.on("console", (m) => {
    if (m.type() === "error") pageErrors.push(m.text());
  });
  await page.evaluateOnNewDocument(
    (fx, cfg) => {
      window.__CALLS__ = [];
      window.__CLIPBOARD__ = null;
      localStorage.clear();
      localStorage.setItem("gitswitch:history.repo", JSON.stringify(fx.repo));
      localStorage.setItem("gitswitch:repo.path", JSON.stringify(fx.repo));
      if (cfg.account) localStorage.setItem("gitswitch:repo.account", JSON.stringify(cfg.account));
      if (cfg.persist) for (const [k, v] of Object.entries(cfg.persist)) localStorage.setItem(`gitswitch:${k}`, JSON.stringify(v));
      // Headless Chrome has no clipboard permission; record what the app tried to copy.
      try {
        Object.defineProperty(navigator, "clipboard", {
          configurable: true,
          value: { writeText: (s) => { window.__CLIPBOARD__ = s; return Promise.resolve(); } },
        });
      } catch {}
      const done = (headline) =>
        Promise.resolve({ ok: true, headline, detail: "", status: cfg.status });
      const act = (cmd) => {
        if (cfg.hang) return new Promise(() => {}); // never settles
        const r = cfg.refuse[cmd];
        if (r) {
          return Promise.resolve({
            ok: false,
            headline: r.headline ?? "Refused",
            detail: r.detail ?? "",
            refusal: { code: r.code, message: r.message ?? `refused: ${r.code}` },
            tree: r.tree,
            status: cfg.status,
          });
        }
        return done(cmd.replace("changes_", "").replace(/_/g, " ") + " ok");
      };
      window.__TAURI_INTERNALS__ = {
        invoke: (cmd, args) => {
          window.__CALLS__.push({ cmd, args });
          switch (cmd) {
            // The Changes tab is mounted beside History now (hidden): its
            // cards need quiet answers, never a null.
            case "get_profiles":
              return Promise.resolve([]);
            case "changes_push_state":
              return Promise.resolve({ ...cfg.status.push, blocked: false, repo_blocked: false, reason: "Pushing is allowed from this folder." });
            case "changes_push_identity":
              return Promise.resolve(null);
            case "changes_submodule_statuses":
            case "changes_stash_list":
            case "console_entries":
              return Promise.resolve([]);
            case "changes_lfs_files":
              return Promise.resolve({ installed: true, version: "git-lfs/3.8.0", uses_lfs: false, filters_configured: true, files: [], folders: [], total: 0, present: 0, missing: 0, total_bytes: 0, missing_bytes: 0, sizes_known: true, truncated: 0, folders_truncated: 0, summary: "This repository doesn't use Git LFS." });
            case "changes_lfs_progress":
            case "clone_progress":
              return Promise.resolve(null);
            case "history_list_repos":
              return cfg.reposDelay ? new Promise((res) => setTimeout(() => res(cfg.repos ?? fx.repos), cfg.reposDelay)) : Promise.resolve(cfg.repos ?? fx.repos);
            case "history_branches":
              return Promise.resolve(cfg.branches);
            case "history_page":
              // A test changes what a given page holds by setting window.__PAGES__[offset].
              return Promise.resolve(window.__PAGES__?.[args.offset] ?? cfg.historyPage ?? fx.historyPage);
            case "history_authors":
              return Promise.resolve(cfg.authors ?? fx.authors);
            case "history_sync_status":
              return Promise.resolve(fx.syncStatus);
            case "history_commit_detail":
              return Promise.resolve(cfg.detail ?? (args.hash === fx.mergeHash ? fx.detailMerge : fx.detail));
            case "history_branch_merges":
              return Promise.resolve({ merged_into: [], merges: [] });
            case "history_commit_file_diff":
              return Promise.resolve({ ...fx.diffText, path: args.path });
            case "history_resolve":
              if (args.text === "nope") return Promise.resolve(null);
              if (args.text === fx.mergeHash) return Promise.resolve(cfg.resolvedMerge);
              return Promise.resolve({ ...cfg.resolved, input: args.text });
            case "history_fetch":
              return Promise.resolve({ message: "fetched", head_unchanged: true, worktree_unchanged: true, local_branches_unchanged: true, updated_remote_refs: 0 });
            case "changes_repo_status":
              return Promise.resolve(cfg.statusEchoesPath ? { ...cfg.status, path: args.repoPath } : cfg.status);
            case "history_head":
              return Promise.resolve({ branch: cfg.status.branch, detached: cfg.status.detached, unborn: cfg.status.unborn });
            case "changes_submodules":
              return Promise.resolve((cfg.submodulesByPath && cfg.submodulesByPath[args.repoPath]) ?? cfg.submodules);
            case "history_peek":
              return Promise.resolve(cfg.peek);
            case "changes_detach":
            case "changes_reset":
            case "changes_create_branch":
            case "changes_revert":
            case "changes_cherry_pick":
              return act(cmd);
            case "full_clone":
              if (cfg.cloneResult) return Promise.resolve(cfg.cloneResult);
              break;
            case "repo_accounts":
              if (cfg.remote) return Promise.resolve([{ key: "gh:octo", label: "octo", source: "GitHub CLI" }]);
              break;
            case "list_remote_repos_page":
              if (cfg.remote) return Promise.resolve({ listing: { account: args.account, login: "octo", suggested_profile_id: null, repos: cfg.remote.repos, truncated: false, sso_hidden_orgs: 0 }, page: args.page, per_page: args.perPage, has_more: false });
              break;
            case "local_clone_index":
              if (cfg.remote) return Promise.resolve({ clones: cfg.remote.clones, owner_profiles: {}, org_ssh_users: {} });
              break;
            case "path_exists":
              if (cfg.remote) return Promise.resolve(cfg.remote.pathExists ?? true);
              break;
          }
          switch (cmd) {
            default:
              // Remembered so a crash can name the command that got a null.
              (window.__UNMOCKED__ = window.__UNMOCKED__ || []).push(cmd);
              return Promise.resolve(null);
          }
        },
      };
    },
    {
      repo: REPO,
      repos: REPOS,
      historyPage: HISTORY_PAGE,
  authors: AUTHORS,
      syncStatus: SYNC_STATUS,
      detail: COMMIT_DETAIL,
      detailMerge: COMMIT_DETAIL_MERGE,
      mergeHash: C3,
      diffText: DIFF_TEXT,
    },
    cfg
  );
  await page.goto(`${server.url}/repo/history`, { waitUntil: "networkidle0" });
  try {
    await page.waitForFunction(() => document.body.innerText.includes("All branches"), { timeout: 5000 });
  } catch (e) {
    // Say what the page shows instead, so a moved anchor is diagnosable.
    console.error("the History tab never showed 'All branches'; the page reads:\n" + (await page.evaluate(() => document.body.innerText.slice(0, 1500))));
    if (pageErrors.length) console.error("page errors:\n" + pageErrors.slice(0, 5).join("\n"));
    const unmocked = await page.evaluate(() => [...new Set(window.__UNMOCKED__ || [])]);
    if (unmocked.length) console.error("commands answered with null by the mock: " + unmocked.join(", "));
    throw e;
  }
  return page;
}

const text = (page) => page.evaluate(() => document.body.innerText);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const calls = (page, cmd) => page.evaluate((c) => window.__CALLS__.filter((x) => x.cmd === c), cmd);
const lastCall = async (page, cmd) => (await calls(page, cmd)).at(-1) ?? null;

/** Find a button by its visible label — an exact match first, then a prefix ("Go" must not pick "Go back…"). */
async function button(page, label) {
  const handles = await page.$$("button");
  const labels = await Promise.all(handles.map((h) => h.evaluate((el) => el.innerText.trim())));
  let i = labels.findIndex((l) => l === label);
  if (i < 0) i = labels.findIndex((l) => l.startsWith(label));
  return i < 0 ? null : handles[i];
}

/** Every button carrying exactly this label. */
const buttonCount = (page, label) =>
  page.evaluate((label) => [...document.querySelectorAll("button")].filter((b) => b.innerText.trim() === label).length, label);

/** Click a button inside the <li> whose text mentions `rowText`. */
async function clickInRow(page, rowText, pick) {
  return page.evaluate(
    (rowText, pick) => {
      const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes(rowText));
      if (!row) return false;
      const buttons = [...row.querySelectorAll("button")];
      const b =
        pick.title !== undefined
          ? buttons.find((x) => x.title === pick.title)
          : pick.text !== undefined
            ? buttons.find((x) => x.innerText.trim() === pick.text)
            : buttons[pick.index];
      if (!b) return false;
      b.click();
      return true;
    },
    rowText,
    pick
  );
}

/** The "Do it" button of the chooser row whose text mentions `rowText`: disabled flag and title. */
const rowState = (page, rowText) =>
  page.evaluate((rowText) => {
    const row = [...document.querySelectorAll('[role="radiogroup"][aria-label="Go back mode"] li')].find((r) =>
      r.innerText.includes(rowText)
    );
    if (!row) return null;
    const b = [...row.querySelectorAll("button")].find((x) => x.innerText.trim() === "Do it");
    return b ? { disabled: b.disabled, title: b.title } : null;
  }, rowText);

/** Open the detail panel for a commit through "Go to commit". */
async function openPanel(page, hash) {
  await page.click('input[aria-label="Go to commit"]', { clickCount: 3 });
  await page.type('input[aria-label="Go to commit"]', hash);
  await (await button(page, "Go")).click();
  await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Copy hash"), { timeout: 4000 });
}

const panelOpen = (page) =>
  page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Copy hash"));

/** Open the "go back" chooser and wait until the resolve round-trip has settled. */
async function openChooser(page) {
  await (await button(page, "Go back to this commit…")).click();
  await page.waitForSelector('[role="radiogroup"][aria-label="Go back mode"]', { timeout: 4000 });
  await page.waitForFunction(() => !document.body.innerText.includes("Checking where this commit sits"), { timeout: 4000 });
  await sleep(50);
}

const server = await serve();
const browser = await launch();
const open = (opts) => openHistoryPage(browser, server, opts);

try {
  // -----------------------------------------------------------------
  section("People: who commits here, and when");
  {
    const page = await open();
    await page.waitForFunction(() => document.body.innerText.includes("People"), { timeout: 4000 });
    const s = await text(page);
    ok("the people list was read for this repository and revision", (await calls(page, "history_authors")).length >= 1);
    ok("every row shows the date AND the time", /\d{4}-\d\d-\d\d \d\d:\d\d/.test(s), s.slice(0, 300));
    // The author's time is what a row shows (it survives a rebase); the committer's is in the title.
    const local = (iso) =>
      page.evaluate((iso) => {
        const d = new Date(iso);
        const p = (n) => String(n).padStart(2, "0");
        return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
      }, iso);
    const authored = await local("2026-09-20T09:30:00+02:00");
    const committed = await local("2026-09-20T10:00:00+02:00");
    const stamps = await page.evaluate(() => [...document.querySelectorAll("span.tabular-nums[title]")].filter((e) => /^\d{4}-\d\d-\d\d \d\d:\d\d$/.test(e.innerText.trim())).map((e) => ({ text: e.innerText.trim(), title: e.title })));
    ok("  it is the author's time", stamps.length > 0 && stamps.every((x) => x.text === authored) && !stamps.some((x) => x.text === committed), JSON.stringify(stamps.slice(0, 2)));
    ok("  with the committer's time in the title", stamps.every((x) => x.title.includes("committed 2026-09-20T10:00:00+02:00")), JSON.stringify(stamps.slice(0, 1)));
    ok("  everyone is the default, with the total", s.includes("Everyone · 8"));
    const pick = async (trigger, option) => {
      await page.evaluate((t) => [...document.querySelectorAll("button")].find((b) => b.innerText.includes(t)).click(), trigger);
      await page.waitForFunction((o) => [...document.querySelectorAll("button")].some((b) => b.innerText.includes(o)), { timeout: 4000 }, option);
      await page.evaluate((o) => [...document.querySelectorAll("button")].find((b) => b.innerText.includes(o)).click(), option);
    };
    await pick("Everyone · 8", "Other Person · 1");
    await page.waitForFunction(() => document.body.innerText.includes("by Other Person"), { timeout: 4000 });
    const last = await lastCall(page, "history_page");
    ok("choosing a person re-reads the page filtered by their email, from the first page", last && JSON.stringify(last.args.authors) === '["other@example.test"]' && last.args.offset === 0, JSON.stringify(last && last.args));
    ok("  and says so in the summary line", (await text(page)).includes("by Other Person"));
    // Open the list again: the bot is marked, and a preset leaves bots out.
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("Other Person · 1")).click());
    await page.waitForFunction(() => document.body.innerText.includes("People only (hide bots and CI) · 3"), { timeout: 4000 });
    const opened = await text(page);
    ok("  a bot is marked as one in the list", opened.includes("github-actions[bot] (bot) · 5"));
    ok("  People only is offered because a bot commits here, with the human total", opened.includes("People only (hide bots and CI) · 3"));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("People only (hide bots and CI) · 3")).click());
    await page.waitForFunction(() => document.body.innerText.includes("by people only"), { timeout: 4000 });
    const ppl = await lastCall(page, "history_page");
    ok("  choosing it sends every human's email and no bot", ppl && JSON.stringify(ppl.args.authors) === '["me@personal.test","other@example.test"]' && ppl.args.offset === 0, JSON.stringify(ppl && ppl.args));
    await page.close();
  }
  {
    // Which commits are pushed: a row no remote-tracking branch has carries a mark, and a toggle lists only those.
    const local = { ...HISTORY_PAGE, commits: [{ ...HISTORY_PAGE.commits[0], refs: ["HEAD -> main"], unpushed: true }, ...HISTORY_PAGE.commits.slice(1)] };
    const page = await openHistoryPage(browser, server, { historyPage: local });
    await page.waitForFunction(() => document.body.innerText.includes("not pushed"), { timeout: 4000 });
    const marked = await page.evaluate(() => [...document.querySelectorAll("button")].filter((b) => b.innerText.includes("not pushed")).map((b) => b.innerText.replace(/\s+/g, " ")));
    ok("a commit no remote has carries a 'not pushed' mark on its row, and only that row", marked.length === 1 && marked[0].includes("Tidy the settings page"), JSON.stringify(marked));
    const box = await page.$('input[aria-label="Only commits not pushed yet"]');
    ok("  the list offers to show only such commits", box !== null);
    await box.evaluate((el) => el.click());
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "history_page").some((c) => c.args.unpushedOnly === true), { timeout: 4000 });
    const only = await lastCall(page, "history_page");
    ok("  which re-reads the page from the start with only what no remote has", only && only.args.unpushedOnly === true && only.args.offset === 0, JSON.stringify(only && only.args));
    ok("  and the summary line says so", (await text(page)).includes("not pushed yet"));
    await page.close();
  }
  {
    // No remote-tracking branch at all: nothing is known to be pushed, and no row is marked.
    const none = { ...HISTORY_PAGE, remote_refs: false, commits: HISTORY_PAGE.commits.map((c) => ({ ...c, unpushed: true })) };
    const page = await openHistoryPage(browser, server, { historyPage: none });
    await page.waitForFunction(() => document.body.innerText.includes("No remote-tracking branch here"), { timeout: 4000 });
    const marked = await page.evaluate(() => [...document.querySelectorAll("button")].filter((b) => b.innerText.includes("not pushed")).length);
    ok("without a remote-tracking branch the page says nothing is known to be pushed, and marks no row", marked === 0);
    await page.close();
  }
  {
    // A repository without submodules, opened with the toggle left on from another one.
    const page = await open({ persist: { "history.hidePointerOnly": true } });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "history_page"), { timeout: 4000 });
    const first = await lastCall(page, "history_page");
    ok("a toggle left on elsewhere hides nothing where there are no submodules", first && first.args.hidePointerOnly === false, JSON.stringify(first && first.args));
    ok("  and is not offered there", (await page.$('input[aria-label="Hide pointer-only commits"]')) === null);
    await page.close();
  }
  {
    // With the repository's account selected, "Just me" is one of the people.
    const page = await open({ account: "p1" });
    await page.waitForFunction(() => document.body.innerText.includes("Everyone · 8"), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("Everyone · 8")).click());
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Just me · 2"), { timeout: 4000 });
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Just me · 2").click());
    await page.waitForFunction(() => document.body.innerText.includes("by me"), { timeout: 4000 });
    const mine = await lastCall(page, "history_page");
    ok("Just me filters by the account's own address, from the first page", mine && JSON.stringify(mine.args.authors) === '["me@personal.test"]' && mine.args.offset === 0, JSON.stringify(mine && mine.args));
    await page.close();
  }
  {
    // "Just me", left on from the last visit — and the people are read before the accounts are.
    const page = await open({ account: "p1", persist: { "history.author": "me" }, reposDelay: 700 });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "history_authors"), { timeout: 4000 });
    await page.waitForFunction(() => document.body.innerText.includes("by me"), { timeout: 4000 }).catch(() => {});
    await sleep(900);
    const kept = await page.evaluate(() => JSON.parse(localStorage.getItem("gitswitch:history.author")));
    ok("Just me, left on from the last visit, survives the people arriving before the accounts", kept === "me", JSON.stringify(kept));
    const mine = await lastCall(page, "history_page");
    ok("  and filters by the account's address once that is known", mine && JSON.stringify(mine.args.authors) === '["me@personal.test"]', JSON.stringify(mine && mine.args));
    ok("  the summary line says so", (await text(page)).includes("by me"));
    const pages = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "history_page").map((c) => c.args.authors));
    ok("  nobody else's commits were shown as mine meanwhile: the page waited for the accounts", pages.length === 1 && JSON.stringify(pages[0]) === '["me@personal.test"]', JSON.stringify(pages));
    await page.close();
  }
  {
    // The older checkbox ("Only commits authored by this account") waits for the accounts the same way.
    const page = await open({ account: "p1", persist: { "history.onlyMine": true }, reposDelay: 700 });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "history_page"), { timeout: 4000 });
    await sleep(300);
    const pages = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "history_page").map((c) => c.args.authors));
    ok("the account-only checkbox also waits for the accounts: one request, filtered", pages.length === 1 && JSON.stringify(pages[0]) === '["me@personal.test"]', JSON.stringify(pages));
    await page.close();
  }
  {
    // An account whose "address" is not an address (an import's `sam <sam>`): "me" is asked for as that whole identity.
    const repos = [{ path: REPO, name: "gitswitch", profile_id: "p1", profile_name: "Sam", profile_email: "sam" }];
    const people = [{ name: "Sam", email: "sam", commits: 2 }, { name: "Samantha Lee", email: "samantha@x.test", commits: 1 }];
    const page = await open({ account: "p1", repos, authors: people, persist: { "history.author": "me" } });
    await page.waitForFunction(() => document.body.innerText.includes("by me"), { timeout: 4000 });
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "history_page" && c.args.authors && c.args.authors[0] === "Sam <sam>"), { timeout: 4000 }).catch(() => {});
    const me = await lastCall(page, "history_page");
    ok("Just me for an account without an @ sends the whole identity, not a word everyone's name may contain", me && JSON.stringify(me.args.authors) === '["Sam <sam>"]', JSON.stringify(me && me.args.authors));
    await page.close();
  }
  {
    // With no account selected there is no "me" here: the preset is dropped, as before.
    const page = await open({ persist: { "history.author": "me" }, reposDelay: 300 });
    await page.waitForFunction(() => JSON.parse(localStorage.getItem("gitswitch:history.author")) === "", { timeout: 4000 }).catch(() => {});
    const kept = await page.evaluate(() => JSON.parse(localStorage.getItem("gitswitch:history.author")));
    ok("  with no account selected, a Just me left on elsewhere is still let go", kept === "", JSON.stringify(kept));
    await page.close();
  }
  {
    // People whose commits carry no address, or one that is not an address, are picked by their whole identity.
    const people = [
      { name: "Sam", email: "", commits: 2 },
      { name: "Samantha Lee", email: "samantha@x.test", commits: 1 },
      { name: "jdoe", email: "jdoe", commits: 1 },
    ];
    const page = await open({ authors: people });
    await page.waitForFunction(() => document.body.innerText.includes("Everyone · 4"), { timeout: 4000 });
    const pick = async (trigger, option) => {
      await page.evaluate((t) => [...document.querySelectorAll("button")].find((b) => b.innerText.includes(t)).click(), trigger);
      await page.waitForFunction((o) => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === o), { timeout: 4000 }, option);
      await page.evaluate((o) => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === o).click(), option);
    };
    await pick("Everyone · 4", "Sam · 2");
    await page.waitForFunction(() => document.body.innerText.includes("by Sam"), { timeout: 4000 });
    const sam = await lastCall(page, "history_page");
    ok("a person with no address is asked for by their whole identity, not a name that others contain", sam && JSON.stringify(sam.args.authors) === '["Sam <>"]', JSON.stringify(sam && sam.args));
    await pick("Sam · 2", "jdoe · 1");
    await page.waitForFunction(() => document.body.innerText.includes("by jdoe"), { timeout: 4000 });
    const jd = await lastCall(page, "history_page");
    ok("  and so is one whose address is not an address", jd && JSON.stringify(jd.args.authors) === '["jdoe <jdoe>"]', JSON.stringify(jd && jd.args));
    await pick("jdoe · 1", "Samantha Lee · 1");
    await page.waitForFunction(() => document.body.innerText.includes("by Samantha Lee"), { timeout: 4000 });
    const sl = await lastCall(page, "history_page");
    ok("  a real address is still sent as the address", sl && JSON.stringify(sl.args.authors) === '["samantha@x.test"]', JSON.stringify(sl && sl.args));
    await page.close();
  }

  section("Go to commit");
  {
    const page = await open();
    ok("the header has a Go to commit field", (await page.$('input[aria-label="Go to commit"]')) !== null);
    ok("with a Go button", (await button(page, "Go")) !== null);
    await openPanel(page, C2);
    const s = await text(page);
    ok("a hash opens the panel showing the subject", s.includes(COMMIT_DETAIL.subject) && s.includes(C2));
    const r = await lastCall(page, "history_resolve");
    ok("  after asking the backend to resolve the text", r?.args.repoPath === REPO && r?.args.text === C2);
    const d = await lastCall(page, "history_commit_detail");
    ok("  and reading the detail of the resolved oid", d?.args.hash === C2);
    await page.click('button[aria-label="Close"]');
    await sleep(100);
    ok("the close button closes it", !(await panelOpen(page)));

    await page.click('input[aria-label="Go to commit"]', { clickCount: 3 });
    await page.type('input[aria-label="Go to commit"]', "nope");
    await (await button(page, "Go")).click();
    await page.waitForFunction(() => document.body.innerText.includes("No commit matches"), { timeout: 4000 });
    ok('an unknown ref says No commit matches "nope"', (await text(page)).includes('No commit matches "nope" in this repository.'));
    ok("  without opening a panel", !(await panelOpen(page)));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("The actions row");
  {
    const page = await open();
    await openPanel(page, C2);
    for (const label of ["Go back to this commit…", "Start a branch here", "Undo this commit (revert)", "Cherry-pick onto main", "Copy hash"]) {
      ok(`the panel offers "${label}"`, (await buttonCount(page, label)) === 1);
    }
    await page.click('button[aria-label="Close"]');
    await sleep(100);
    // The branch list repeats the subject, so pick the row by its short hash.
    await page.evaluate((s) => [...document.querySelectorAll("button")].find((b) => b.innerText.includes(s)).click(), HISTORY_PAGE.commits[0].short);
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Copy hash"), { timeout: 4000 });
    ok("clicking a commit row still opens the panel, with the same actions", (await buttonCount(page, "Go back to this commit…")) === 1);
    await (await button(page, "Copy hash")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Copied"), { timeout: 4000 });
    ok("Copy hash toasts Copied <short>…", (await text(page)).includes(`Copied ${SHORT}…`));
    ok("  and the full hash reached the clipboard", (await page.evaluate(() => window.__CLIPBOARD__)) === C2);
    ok("the changed files are still listed with their counts", (await text(page)).includes("src/app.ts") && (await text(page)).includes("+3"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Go back: look at it / move the branch");
  {
    const page = await open();
    await openPanel(page, C2);
    await openChooser(page);
    const rows = await page.evaluate(() =>
      [...document.querySelectorAll('[role="radiogroup"][aria-label="Go back mode"] li')].map((li) => li.innerText)
    );
    ok("the chooser has four rows", rows.length === 4, rows.join(" | "));
    ok('  "Look at it" first', rows[0]?.startsWith("Look at it"));
    ok("  each with a Do it button", (await buttonCount(page, "Do it")) === 4);
    ok("  the detached-HEAD row explains nothing is lost", rows[0]?.includes("detached HEAD") && rows[0]?.includes("Nothing is lost"));
    ok("  the branch rows name the current branch", rows[1]?.includes("Move main here") && rows[3]?.includes("drop everything after it"));
    ok("opening it resolved the commit", (await calls(page, "history_resolve")).length >= 2);
    // The chooser itself reads the repo status (it needs the dirty-tree facts);
    // the page's own HEAD question is the cheap one.
    ok("  and read where HEAD is", (await calls(page, "history_head")).length >= 1);
    ok("no confirm dialog is stacked on the chooser", !(await text(page)).includes("Move main back to"));

    ok("Do it on Look at it", await clickInRow(page, "Look at it", { text: "Do it" }));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_detach"), { timeout: 4000 });
    const det = await lastCall(page, "changes_detach");
    ok("  sends changes_detach with the repo and the commit", det?.args.repoPath === REPO && det?.args.target === C2);
    await page.waitForFunction(() => ![...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Copy hash"), { timeout: 4000 });
    ok("  then the panel closes", !(await panelOpen(page)));
    await page.close();
  }
  {
    const page = await open();
    await openPanel(page, C2);
    await openChooser(page);
    ok("Do it on the mixed row", await clickInRow(page, "keep later changes as uncommitted", { text: "Do it" }));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_reset"), { timeout: 4000 });
    const r = await lastCall(page, "changes_reset");
    ok('  sends changes_reset mode "mixed"', r?.args.mode === "mixed" && r?.args.target === C2 && r?.args.repoPath === REPO, JSON.stringify(r?.args));
    // A mixed reset leaves the working tree alone, so nothing needs setting aside — even on a dirty tree.
    ok("  without a stash, dirty tree or not (nothing in the tree is touched)", r?.args.stashFirst === false);
    await page.close();
  }
  {
    const page = await open();
    await openPanel(page, C2);
    await openChooser(page);
    ok("Do it on the soft row", await clickInRow(page, "keep later changes staged", { text: "Do it" }));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_reset"), { timeout: 4000 });
    const r = await lastCall(page, "changes_reset");
    ok('  sends changes_reset mode "soft"', r?.args.mode === "soft");
    ok("  without a stash either", r?.args.stashFirst === false);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Go back: drop everything after it asks first");
  {
    const page = await open({ status: STATUS_DIRTY });
    await openPanel(page, C2);
    await openChooser(page);
    ok("Do it on the hard row", await clickInRow(page, "drop everything after it", { text: "Do it" }));
    await page.waitForFunction(() => document.body.innerText.includes("Move main back to"), { timeout: 4000 });
    let s = await text(page);
    ok(`it asks "Move main back to ${SHORT}?"`, s.includes(`Move main back to ${SHORT}?`));
    ok("  naming the 4 commits that would leave", s.includes("The 4 commits after it leave the branch."));
    ok("  and the backup branch", s.includes("gitswitch-before-reset-<time>"));
    ok("  nothing was reset yet", (await calls(page, "changes_reset")).length === 0);
    const cb = await page.$('input[aria-label="Stash uncommitted changes first"]');
    ok("a dirty tree gets the stash checkbox", cb !== null);
    ok("  checked by default", await cb.evaluate((el) => el.checked));
    ok("  with no warning while it is checked", !s.includes("thrown away"));
    await cb.click();
    await sleep(50);
    s = await text(page);
    ok("  unchecking it warns the changes are thrown away", s.includes("Without stashing, your uncommitted changes are thrown away. That cannot be undone."));
    await (await button(page, "Keep things as they are")).click();
    await sleep(100);
    ok("Keep things as they are closes the dialog without resetting", !(await text(page)).includes("Move main back to") && (await calls(page, "changes_reset")).length === 0);
    ok("  and the chooser is still there", (await page.$('[role="radiogroup"][aria-label="Go back mode"]')) !== null);

    await clickInRow(page, "drop everything after it", { text: "Do it" });
    await page.waitForFunction(() => document.body.innerText.includes("Move main back to"), { timeout: 4000 });
    await (await button(page, "Move main back")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_reset"), { timeout: 4000 });
    const r = await lastCall(page, "changes_reset");
    ok('Move main back sends changes_reset mode "hard"', r?.args.mode === "hard" && r?.args.target === C2, JSON.stringify(r?.args));
    ok("  without stashing, as unchecked", r?.args.stashFirst === false);
    await page.close();
  }
  {
    const page = await open({ status: STATUS_CLEAN });
    await openPanel(page, C2);
    await openChooser(page);
    await clickInRow(page, "drop everything after it", { text: "Do it" });
    await page.waitForFunction(() => document.body.innerText.includes("Move main back to"), { timeout: 4000 });
    ok("a clean tree has no stash checkbox", (await page.$('input[aria-label="Stash uncommitted changes first"]')) === null);
    await (await button(page, "Move main back")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_reset"), { timeout: 4000 });
    ok("  and resets without a stash", (await lastCall(page, "changes_reset"))?.args.stashFirst === false);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Go back: commits already on a remote branch");
  {
    const page = await open({ resolved: RESOLVED_PUSHED });
    await openPanel(page, C2);
    await openChooser(page);
    const s = await text(page);
    ok("the amber note explains a force-push would be needed", s.includes("Moving main back would drop commits already on a remote branch, which needs a force-push — GitSwitch never does that."));
    ok('  offering "Undo this commit (revert)"', (await buttonCount(page, "Undo this commit (revert)")) === 2);
    ok('  and "Start a branch here"', (await buttonCount(page, "Start a branch here")) === 2);
    const look = await rowState(page, "Look at it");
    ok("Look at it stays available", look && !look.disabled);
    for (const row of ["keep later changes as uncommitted", "keep later changes staged", "drop everything after it"]) {
      const st = await rowState(page, row);
      ok(`  "${row}" is disabled`, st && st.disabled, JSON.stringify(st));
    }
    // The note's own buttons lead to the same places as the actions row.
    await page.evaluate(() => {
      const note = [...document.querySelectorAll("div")].find((d) => d.innerText.startsWith("Moving main back would drop"));
      [...note.querySelectorAll("button")].find((b) => b.innerText.trim() === "Start a branch here").click();
    });
    await page.waitForSelector('input[aria-label="Branch name"]', { timeout: 4000 });
    ok("  the note's Start a branch here opens the branch form", true);
    await page.close();
  }
  {
    // The backend can say it too — e.g. the upstream moved since the page resolved.
    const page = await open({ refuse: { changes_reset: { code: "would-drop-pushed", message: "main is already on origin/main up to that point." } } });
    await openPanel(page, C2);
    await openChooser(page);
    await clickInRow(page, "keep later changes as uncommitted", { text: "Do it" });
    await page.waitForFunction(() => document.body.innerText.includes("GitSwitch never does that"), { timeout: 4000 });
    ok("a would-drop-pushed refusal shows the same note", true);
    ok("  the panel stays open", await panelOpen(page));
    const st = await rowState(page, "keep later changes staged");
    ok("  and the branch rows are now disabled", st && st.disabled);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Go back: a commit that is not behind the branch");
  {
    const page = await open({ resolved: RESOLVED_NOT_CONTAINED });
    await openPanel(page, C2);
    await openChooser(page);
    for (const row of ["keep later changes as uncommitted", "keep later changes staged", "drop everything after it"]) {
      const st = await rowState(page, row);
      ok(`"${row}" is disabled, saying it is not behind main`, st && st.disabled && st.title.includes("not behind") && st.title.includes("main"), JSON.stringify(st));
    }
    const look = await rowState(page, "Look at it");
    ok("Look at it is still possible", look && !look.disabled);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Undo this commit (revert)");
  {
    const page = await open();
    await openPanel(page, C2);
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForFunction((s) => document.body.innerText.includes(`Revert ${s}?`), { timeout: 4000 }, SHORT);
    let s = await text(page);
    ok(`it asks "Revert ${SHORT}?"`, s.includes(`Revert ${SHORT}?`));
    ok("  explaining a new commit on main undoes it", s.includes("Makes a new commit on main that undoes what this one did. Nothing is rewritten — the original stays in history."));
    ok("  and where conflicts go", s.includes("If it conflicts, you resolve it on the Changes page."));
    ok("  with no mainline question for a plain commit", (await page.$('[role="radiogroup"][aria-label="Mainline"]')) === null);
    await (await button(page, "Not now")).click();
    await sleep(100);
    ok("Not now closes it without reverting", !(await text(page)).includes(`Revert ${SHORT}?`) && (await calls(page, "changes_revert")).length === 0);
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForFunction((s) => document.body.innerText.includes(`Revert ${s}?`), { timeout: 4000 }, SHORT);
    await (await button(page, "Revert")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_revert"), { timeout: 4000 });
    const r = await lastCall(page, "changes_revert");
    ok("Revert sends changes_revert with mainline null", r?.args.target === C2 && r?.args.mainline === null && r?.args.repoPath === REPO, JSON.stringify(r?.args));
    await page.close();
  }
  {
    const page = await open();
    await openPanel(page, C3);
    ok("a merge commit's panel says so", (await text(page)).includes("merge commit"));
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForSelector('[role="radiogroup"][aria-label="Mainline"]', { timeout: 4000 });
    const labels = await page.evaluate(() =>
      [...document.querySelectorAll('[role="radiogroup"][aria-label="Mainline"] label')].map((l) => l.innerText.replace(/\s+/g, " ").trim())
    );
    ok("a merge asks which side to keep", labels.length === 2, labels.join(" | "));
    ok(`  "Keep ${PARENTS[0].short} ${PARENTS[0].subject}"`, labels[0] === `Keep ${PARENTS[0].short} ${PARENTS[0].subject}`);
    ok(`  "Keep ${PARENTS[1].short} ${PARENTS[1].subject}"`, labels[1] === `Keep ${PARENTS[1].short} ${PARENTS[1].subject}`);
    const checked = await page.evaluate(() => [...document.querySelectorAll('[role="radiogroup"][aria-label="Mainline"] input')].map((i) => i.checked));
    ok("  the first parent is the default", checked[0] === true && checked[1] === false);
    await (await button(page, "Revert")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_revert"), { timeout: 4000 });
    const r = await lastCall(page, "changes_revert");
    ok("Revert sends mainline 1", r?.args.target === C3 && r?.args.mainline === 1, JSON.stringify(r?.args));
    await page.close();
  }
  {
    const page = await open({
      refuse: { changes_revert: { code: "merge-commit-needs-mainline", message: "This is a merge: say which parent to keep.", tree: treeOutcome({ parents: PARENTS }) } },
    });
    await openPanel(page, C2);
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForFunction((s) => document.body.innerText.includes(`Revert ${s}?`), { timeout: 4000 }, SHORT);
    await (await button(page, "Revert")).click();
    await page.waitForSelector('[role="radiogroup"][aria-label="Mainline"]', { timeout: 4000 });
    const s = await text(page);
    ok("a merge-commit-needs-mainline refusal reopens the confirm with the parents", s.includes(`Keep ${P1.slice(0, 7)}`) && s.includes(`Keep ${P2.slice(0, 7)}`));
    ok("  the panel is still open behind it", await panelOpen(page));
    await page.evaluate(() => [...document.querySelectorAll('[role="radiogroup"][aria-label="Mainline"] input')][1].click());
    await sleep(50);
    await (await button(page, "Revert")).click();
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "changes_revert").length === 2, { timeout: 4000 });
    ok("  choosing the second parent sends mainline 2", (await lastCall(page, "changes_revert"))?.args.mainline === 2);
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Cherry-pick");
  {
    const page = await open();
    await openPanel(page, C2);
    await (await button(page, "Cherry-pick onto main")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_cherry_pick"), { timeout: 4000 });
    const r = await lastCall(page, "changes_cherry_pick");
    ok("Cherry-pick onto main sends changes_cherry_pick", r?.args.target === C2 && r?.args.repoPath === REPO, JSON.stringify(r?.args));
    ok("  with no confirm dialog", (await calls(page, "changes_cherry_pick")).length === 1);
    await page.close();
  }
  {
    const page = await open({ status: STATUS_DETACHED, branches: BRANCHES_DETACHED });
    await openPanel(page, C2);
    const b = await button(page, "Cherry-pick");
    ok("with a detached HEAD the cherry-pick button is disabled", b !== null && (await b.evaluate((el) => el.disabled)));
    ok('  saying "Switch to a branch first"', (await b.evaluate((el) => el.title)) === "Switch to a branch first");
    // A revert makes a commit too, and the backend refuses it the same way — so it is gated the same way.
    const rv = await button(page, "Undo this commit (revert)");
    ok("and so is Undo this commit (revert), for the same reason", rv !== null && (await rv.evaluate((el) => el.disabled && el.title === "Switch to a branch first")));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Start a branch here");
  {
    const page = await open();
    await openPanel(page, C2);
    await (await button(page, "Start a branch here")).click();
    await page.waitForSelector('input[aria-label="Branch name"]', { timeout: 4000 });
    ok("an inline name field appears", true);
    const create = await button(page, "Create");
    ok("  Create waits for a name", await create.evaluate((el) => el.disabled));
    await page.type('input[aria-label="Branch name"]', "fix/lfs-hint");
    await (await button(page, "Create")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_create_branch"), { timeout: 4000 });
    const r = await lastCall(page, "changes_create_branch");
    ok("Create sends changes_create_branch from this commit, switching to it", r?.args.name === "fix/lfs-hint" && r?.args.from === C2 && r?.args.switchTo === true, JSON.stringify(r?.args));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("File diffs inside the panel");
  {
    const page = await open();
    await openPanel(page, C2);
    ok("file rows are buttons", await page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.includes("src/app.ts"))));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("src/app.ts")).click());
    await page.waitForFunction(() => document.body.innerText.includes("@@ -1,3 +1,4 @@"), { timeout: 4000 });
    const r = await lastCall(page, "history_commit_file_diff");
    ok("clicking one loads its diff for this commit", r?.args.hash === C2 && r?.args.path === "src/app.ts" && r?.args.repoPath === REPO, JSON.stringify(r?.args));
    let s = await text(page);
    ok("  and shows the hunk with both line kinds", s.includes("@@ -1,3 +1,4 @@") && s.includes("+added line") && s.includes("-removed line"));
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("README.md")).click());
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "history_commit_file_diff").length === 2, { timeout: 4000 });
    await sleep(100);
    ok("opening another file swaps the diff (one at a time)", (await lastCall(page, "history_commit_file_diff"))?.args.path === "README.md" && (await page.$$("pre")).length <= 2);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("README.md")).click());
    await sleep(100);
    s = await text(page);
    ok("clicking it again collapses the diff", !s.includes("@@ -1,3 +1,4 @@"));
    ok("a binary file's row says binary instead of +- −-", s.includes("logo.png") && s.includes("binary") && !s.includes("+-") && !s.includes("−-"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("The Merge status card follows the branch");
  {
    const page = await open();
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.title === "main").click());
    await page.waitForFunction(() => document.body.innerText.includes("Merge status"), { timeout: 4000 });
    const first = await calls(page, "history_branch_merges");
    ok("selecting a branch asks which branches contain it", first.length === 1 && first[0].args.branch === "main" && first[0].args.repoPath === REPO);
    await openPanel(page, C2);
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForFunction((s) => document.body.innerText.includes(`Revert ${s}?`), { timeout: 4000 }, SHORT);
    await (await button(page, "Revert")).click();
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Open in Changes"), { timeout: 4000 });
    await sleep(200);
    const after = await calls(page, "history_branch_merges");
    ok("  and asks again after an action (a reset or a new branch changes the answer)", after.length === 2 && after[1].args.branch === "main");
    ok("  the card is still there", (await text(page)).includes("Merge status"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("After an action you stay on History");
  {
    const page = await open();
    await openPanel(page, C2);
    const before = (await calls(page, "history_page")).length;
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForFunction((s) => document.body.innerText.includes(`Revert ${s}?`), { timeout: 4000 }, SHORT);
    await (await button(page, "Revert")).click();
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Open in Changes"), { timeout: 4000 });
    ok("the panel closes", !(await panelOpen(page)));
    const s = await text(page);
    ok("a result card shows the outcome", s.includes("revert ok"));
    ok('  with "Open in Changes"', (await buttonCount(page, "Open in Changes")) === 1);
    ok("the branch list is still on screen", s.includes("All branches") && s.includes("feature/lfs"));
    ok("  and the URL is still /history", (await page.evaluate(() => location.pathname)) === "/repo/history");
    await sleep(200);
    ok("branches, sync and the page were re-read", (await calls(page, "history_page")).length > before && (await calls(page, "history_branches")).length >= 2 && (await calls(page, "history_sync_status")).length >= 2);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "dismiss").click());
    await sleep(50);
    ok("dismiss removes the card", (await buttonCount(page, "Open in Changes")) === 0);
    await page.close();
  }
  {
    const page = await open();
    await openPanel(page, C2);
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForFunction((s) => document.body.innerText.includes(`Revert ${s}?`), { timeout: 4000 }, SHORT);
    await (await button(page, "Revert")).click();
    await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Open in Changes"), { timeout: 4000 });
    await (await button(page, "Open in Changes")).click();
    await sleep(200);
    ok("Open in Changes goes to the Changes page for this repo", (await page.evaluate(() => location.pathname)) === "/repo/changes" && (await page.evaluate(() => localStorage.getItem("gitswitch:repo.path"))) === JSON.stringify(REPO));
    await page.close();
  }
  {
    const page = await open({ hang: true });
    await openPanel(page, C2);
    await (await button(page, "Undo this commit (revert)")).click();
    await page.waitForFunction((s) => document.body.innerText.includes(`Revert ${s}?`), { timeout: 4000 }, SHORT);
    await (await button(page, "Revert")).click();
    await sleep(200);
    const b = await button(page, "Cherry-pick onto main");
    ok("while an action runs the other actions are disabled", b !== null && (await b.evaluate((el) => el.disabled)));
    ok("  and the panel says it is working", (await text(page)).includes("Working…"));
    await page.close();
  }

  // -----------------------------------------------------------------
  section("Check GitHub: what is new, without downloading anything");
  {
    const page = await openHistoryPage(browser, server);
    const b = await button(page, "Check GitHub");
    ok("the button is offered next to Fetch", b !== null && (await b.evaluate((x) => x.title)).includes("downloads nothing"));
    await b.click();
    await page.waitForFunction(() => document.body.innerText.includes("Nothing was downloaded"), { timeout: 5000 });
    const s = await text(page);
    ok("the answer names the count and says nothing was downloaded", s.includes("3 new commits on origin/main") && s.includes("Nothing was downloaded"));
    const calls = await page.evaluate(() => window.__CALLS__.map((c) => c.cmd));
    ok("  it asked the backend to peek, not to fetch", calls.includes("history_peek") && !calls.includes("history_fetch"));
    ok("  and offers Fetch now", (await button(page, "Fetch now")) !== null);
    await (await button(page, "Fetch now")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "history_fetch"), { timeout: 5000 });
    ok("Fetch now fetches for real and clears the check", !(await text(page)).includes("Nothing was downloaded"));
    await page.close();
  }
  {
    const page = await openHistoryPage(browser, server, { peek: { branch: "main", upstream: "origin/main", remote: "origin", remote_tip: "e81d1c5", known_tip: "e81d1c5", changed: false, branch_gone: false, new_commits: null, counted_by: null, message: "Nothing new on origin/main since your last fetch." } });
    await (await button(page, "Check GitHub")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Nothing new on origin/main"), { timeout: 5000 });
    ok("when nothing moved it says so, with no Fetch now", (await button(page, "Fetch now")) === null);
    await page.close();
  }

  section("Submodules: a submodule's history from the same page");
  {
    const SUBS = [
      sub({ path: "staging", name: "staging", own_ahead: 2, own_behind: 0, summary: "2 commits of yours not on its upstream" }),
      sub({ path: "harness", name: "harness", own_ahead: 0, own_behind: 1, summary: "1 commit behind its upstream" }),
      sub({ path: "forge-tooling/x", name: "x", initialised: false, listed: false, gitdir_valid: false, state: "unmapped", actual: null, actual_short: null, summary: "not initialised" }),
    ];
    const page = await openHistoryPage(browser, server, { submodules: SUBS, historyPage: HISTORY_PAGE_BUMP });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules:"), { timeout: 5000 });
    ok("a commit that only bumps a submodule pointer says so on its row", (await text(page)).includes("submodule bump: trinity"));
    const hideBox = await page.$('input[aria-label="Hide pointer-only commits"]');
    ok("  and the list offers to hide such commits", hideBox !== null);
    await hideBox.evaluate((el) => el.click());
    await page.waitForFunction(() => window.__CALLS__.filter((c) => c.cmd === "history_page").some((c) => c.args.hidePointerOnly === true), { timeout: 4000 });
    const hidden = await lastCall(page, "history_page");
    ok("  which re-reads the page from the start with the bumps left out", hidden && hidden.args.hidePointerOnly === true && hidden.args.offset === 0, JSON.stringify(hidden && hidden.args));
    await hideBox.evaluate((el) => el.click());
    await page.waitForFunction(() => { const l = window.__CALLS__.filter((c) => c.cmd === "history_page"); return l.length > 0 && l[l.length - 1].args.hidePointerOnly === false; }, { timeout: 4000 });
    let s = await text(page);
    ok("populated, mapped submodules are listed as chips with their own ahead/behind", s.includes("staging ↑2") && s.includes("harness ↓1"));
    ok("  an uninitialised gitlink is not offered", !s.includes("forge-tooling"));
    ok("  the repository itself is what the page shows by default", !s.includes("Browsing inside"));
    const subCallsBefore = await page.evaluate(() => window.__CALLS__.filter((c) => c.args?.repoPath === "/repos/gitswitch/staging").length);
    ok("  nothing was read from the submodule yet", subCallsBefore === 0);
    await (await button(page, "staging ↑2")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Browsing inside"), { timeout: 5000 });
    s = await text(page);
    ok("clicking a chip browses that submodule and says so", s.includes("Browsing inside") && s.includes("staging"));
    const subCalls = await page.evaluate(() => window.__CALLS__.filter((c) => c.args?.repoPath === "/repos/gitswitch/staging").map((c) => c.cmd));
    ok("  branches, status, sync and the commit page are read from the submodule", ["history_branches", "history_head", "history_sync_status", "history_page"].every((c) => subCalls.includes(c)));
    await (await button(page, "Fetch")).click();
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "history_fetch"), { timeout: 5000 });
    const fetched = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "history_fetch").map((c) => c.args.repoPath));
    ok("  Fetch fetches inside the submodule", fetched.length === 1 && fetched[0] === "/repos/gitswitch/staging");
    ok("  the Fetch button says which submodule it fetches", (await (await button(page, "Fetch")).evaluate((b) => b.title)).includes("staging"));
    await (await button(page, "Back to the repository")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Submodules:"), { timeout: 5000 });
    ok("Back to the repository returns to the superproject", !(await text(page)).includes("Browsing inside"));
    const back = await page.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "history_page").map((c) => c.args.repoPath));
    ok("  and re-reads the repository's own commits", back[back.length - 1] === "/repos/gitswitch");
    await page.close();
  }
  {
    const page = await openHistoryPage(browser, server);
    await sleep(400);
    ok("a repository without submodules shows no picker and no chips", !(await text(page)).includes("Submodules:") && (await button(page, "This repository")) === null);
    await page.close();
  }

  {
    // A submodule inside a submodule: from trinity's scope, inner's History
    // opens inner itself as the page's repository — at any depth.
    const trinity = sub({ path: "trinity", name: "trinity", own_ahead: 0, own_behind: 0, summary: "clean" });
    const inner = sub({ path: "inner", name: "inner", own_ahead: 0, own_behind: 0, summary: "clean" });
    const page = await openHistoryPage(browser, server, { statusEchoesPath: true, submodules: [trinity], submodulesByPath: { "/repos/gitswitch": [trinity], "/repos/gitswitch/trinity": [inner] } });
    await page.waitForFunction(() => document.body.innerText.includes("Submodules:"), { timeout: 5000 });
    await (await button(page, "trinity"))?.click();
    await page.waitForFunction(() => document.body.innerText.includes("Browsing inside"), { timeout: 5000 }).catch(() => null);
    await page.evaluate(() => [...document.querySelectorAll('a[role="tab"]')].find((a) => a.innerText.trim().startsWith("Submodules"))?.click());
    await page.waitForFunction(() => [...document.querySelectorAll("tr")].some((r) => r.innerText.includes("inner")), { timeout: 5000 }).catch(() => null);
    const mark = await page.evaluate(() => window.__CALLS__.length);
    await page.evaluate(() => {
      const row = [...document.querySelectorAll("tr")].find((r) => r.innerText.includes("inner"));
      [...(row?.querySelectorAll("button") ?? [])].find((b) => b.innerText.trim() === "History")?.click();
    });
    await page.waitForFunction(() => location.pathname === "/repo/history", { timeout: 5000 }).catch(() => null);
    await sleep(900);
    const st = await page.evaluate((from) => ({
      path: JSON.parse(localStorage.getItem("gitswitch:repo.path") ?? "null"),
      sub: JSON.parse(localStorage.getItem("gitswitch:repo.sub") ?? "null"),
      reads: [...new Set(window.__CALLS__.slice(from).filter((c) => c.cmd === "history_page").map((c) => c.args.repoPath))],
    }), mark);
    ok("from a submodule's scope, a nested submodule's History opens that nested repository itself",
      st.path === "/repos/gitswitch/trinity/inner" && !st.sub && st.reads.includes("/repos/gitswitch/trinity/inner"), JSON.stringify(st));
    await page.close();
  }

  section("Paging: the next fifty, a page that no longer exists, long lists");
  {
    const big = { ...HISTORY_PAGE, total: 120, has_more: true };
    const page = await open({ historyPage: big });
    await page.waitForFunction(() => document.body.innerText.includes("Page 1 of 3"), { timeout: 4000 }).catch(() => null);
    await (await button(page, "Next")).click();
    await page.waitForFunction(() => document.body.innerText.includes("Page 2 of 3"), { timeout: 4000 }).catch(() => null);
    const offsets = (await calls(page, "history_page")).map((c) => c.args.offset);
    ok("Next asks for offset 50 and shows Page 2 of 3", offsets.includes(50) && (await text(page)).includes("Page 2 of 3"), JSON.stringify(offsets));
    // The history shrinks to 30 commits under page 2 (a reset made elsewhere), and the page is read again.
    await page.evaluate((first) => {
      window.__PAGES__ = { 50: { ...first, commits: [], total: 30, offset: 50, has_more: false }, 0: { ...first, total: 30, has_more: false } };
    }, HISTORY_PAGE);
    const mark = (await calls(page, "history_page")).length;
    await (await button(page, "Refresh")).click();
    await page.waitForFunction((m) => window.__CALLS__.filter((c) => c.cmd === "history_page").slice(m).some((c) => c.args.offset === 0), { timeout: 4000 }, mark).catch(() => null);
    await sleep(400);
    const after = (await calls(page, "history_page")).slice(mark).map((c) => c.args.offset);
    const s = await text(page);
    ok("a history that shrank under a later page goes back to a page that exists", after.includes(50) && after.at(-1) === 0 && s.includes("showing 1–3") && !s.includes("51–50"), `${JSON.stringify(after)} | ${s.slice(s.indexOf("All branches"), s.indexOf("All branches") + 80)}`);
    await page.close();
  }
  {
    const many = [BRANCHES[0], ...Array.from({ length: 300 }, (_, i) => ({ ...BRANCHES[1], name: `topic/${i}` }))];
    const page = await open({ branches: many });
    await page.waitForFunction(() => document.querySelectorAll('button[title^="topic/"]').length > 0, { timeout: 4000 }).catch(() => null);
    // Every row of the list (main and the topics): the buttons in the topics' own container.
    const rows = () => page.evaluate(() => document.querySelector('button[title^="topic/"]')?.parentElement.querySelectorAll(":scope > button").length ?? 0);
    ok("a long branch list puts at most 50 rows on screen", (await rows()) === 50, String(await rows()));
    await page.evaluate(() => {
      const list = document.querySelector('button[title^="topic/"]').parentElement;
      list.scrollTop = list.scrollHeight;
    });
    await sleep(300);
    ok("  scrolled to its end, fifty more", (await rows()) === 100, String(await rows()));
    await page.type('input[aria-label="Filter branches"]', "topic/29");
    await sleep(300);
    ok("  and a filter narrows it to the matching names", (await rows()) === 11 && (await text(page)).includes("11 of 301"), String(await rows()));
    await page.close();
  }
  {
    const files = Array.from({ length: 500 }, (_, i) => ({ path: `vendor/lib/file-${i}.js`, added: "1", removed: "0", status: "M" }));
    const page = await open({ detail: { ...COMMIT_DETAIL, files } });
    await openPanel(page, C2);
    const fileRows = () => page.evaluate(() => [...document.querySelectorAll("button[aria-expanded]")].filter((b) => b.innerText.includes("vendor/lib/file-")).length);
    ok("a huge commit lists 200 files and offers the rest", (await fileRows()) === 200 && (await text(page)).includes("Show all 500 files"), String(await fileRows()));
    await (await button(page, "Show all 500 files")).click();
    await sleep(300);
    ok("  which shows them all", (await fileRows()) === 500, String(await fileRows()));
    await page.close();
  }

  section("Picking a repository from a long list");
  {
    const page = await open();
    await page.waitForFunction(() => document.body.innerText.includes("History"), { timeout: 4000 });
    // The page labels neither picker; the repository one is the .w-64 in the header.
    await page.evaluate(() => {
      document.querySelector("div.w-64 button").click();
    });
    await page.waitForFunction(() => !!document.querySelector('input[aria-label="Search repositories"]'), { timeout: 4000 });
    ok("History's repository picker has the same search box", true);
    await page.evaluate(() => {
      const i = document.querySelector('input[aria-label="Search repositories"]');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(i, "argos");
      i.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await sleep(200);
    const rows = await page.evaluate(() => ({
      argos: [...document.querySelectorAll("button")].some((b) => b.innerText.includes("argos")),
      gitswitch: [...document.querySelectorAll('[role], button')].filter((b) => b.innerText.trim().startsWith("gitswitch")).length,
    }));
    ok("  and it narrows to the match", rows.argos);
    await page.close();
  }

  section("A hand-off opens the repository it hands over");
  {
    // An earlier visit: gitswitch selected, the Kirmada filter on, a gitswitch
    // branch remembered. Repositories → Open on argos (Work's) must show argos,
    // under All accounts and on all branches — not stay where it was.
    const remote = {
      repos: [{ full_name: "octo/argos", owner: "octo", name: "argos", owner_is_org: false, description: null, private: true, archived: false, fork: false, default_branch: "main", clone_url: "git@github.com:octo/argos.git", pushed_at: "2026-10-01T00:00:00Z", permission: "write", local_path: "/repos/argos", suggested_profile_id: null }],
      clones: { "octo/argos": "/repos/argos" },
    };
    const page = await open({ account: "p1", persist: { "history.rev": "feature/lfs" }, remote });
    await page.evaluate(() => [...document.querySelectorAll("nav a")].find((a) => a.innerText.trim().startsWith("Repositories"))?.click());
    const listed = await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Open"), { timeout: 6000 }).then(() => true).catch(() => false);
    ok("(the Repositories page lists argos as already here, with Open)", listed);
    const mark = await page.evaluate(() => window.__CALLS__.length);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Open")?.click());
    await page.waitForFunction(() => location.pathname === "/repo/history", { timeout: 5000 }).catch(() => null);
    await sleep(800);
    const state = await page.evaluate((from) => ({
      path: JSON.parse(localStorage.getItem("gitswitch:repo.path") ?? "null"),
      account: JSON.parse(localStorage.getItem("gitswitch:repo.account") ?? "null"),
      pages: window.__CALLS__.slice(from).filter((c) => c.cmd === "history_page").map((c) => `${c.args.repoPath} ${c.args.rev}`),
      where: location.pathname,
    }), mark);
    ok("Repositories → Open lands on that repository's History, even with another account's filter left on",
      state.where === "/repo/history" && state.path === "/repos/argos" && state.account === "", JSON.stringify(state));
    ok("  and reads it on all branches, not on a branch remembered from another repository",
      state.pages.length > 0 && state.pages.every((p) => p === "/repos/argos --all"), JSON.stringify(state.pages));
    await page.close();
  }

  section("Layout");
  // Measure <main>, not documentElement: main has overflow-y-auto, so it
  // scrolls horizontally on its own and the document never reports overflow.
  for (const width of [700, 900, 1280]) {
    const page = await open({ width, resolved: RESOLVED_PUSHED });
    await openPanel(page, C2);
    await openChooser(page);
    await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("src/app.ts")).click());
    await page.waitForFunction(() => document.body.innerText.includes("@@ -1,3 +1,4 @@"), { timeout: 4000 });
    await sleep(200);
    const o = await page.evaluate(() => {
      const m = document.querySelector("main");
      const panel = [...document.querySelectorAll("div")].find((d) => d.className.includes("max-w-3xl") && d.className.includes("overflow-y-auto"));
      return {
        need: m.scrollWidth,
        have: m.clientWidth,
        panelNeed: panel?.scrollWidth ?? 0,
        panelHave: panel?.clientWidth ?? 0,
      };
    });
    ok(`no horizontal overflow of <main> at ${width}px with the panel open`, o.need <= o.have + 1, `main needs ${o.need}px but has ${o.have}px`);
    ok(`  nor inside the panel itself`, o.panelNeed <= o.panelHave + 1, `panel needs ${o.panelNeed}px but has ${o.panelHave}px`);
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}

process.exit(t.done() ? 0 : 1);
