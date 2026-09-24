// Headless checks for the History page's commit actions against a mocked backend.
// Serves the built app and replaces Tauri's invoke bridge before any app code
// runs, so every reset/revert/cherry-pick can be exercised without a real repo.
import { launch, serve, tally, DIST_OK } from "./browser.mjs";
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
} from "./history-fixtures.mjs";

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
  };
  const page = await browser.newPage();
  await page.setViewport({ width: cfg.width, height: 900 });
  await page.evaluateOnNewDocument(
    (fx, cfg) => {
      window.__CALLS__ = [];
      window.__CLIPBOARD__ = null;
      localStorage.clear();
      localStorage.setItem("gitswitch:history.repo", JSON.stringify(fx.repo));
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
            case "history_list_repos":
              return Promise.resolve(fx.repos);
            case "history_branches":
              return Promise.resolve(cfg.branches);
            case "history_page":
              return Promise.resolve(fx.historyPage);
            case "history_sync_status":
              return Promise.resolve(fx.syncStatus);
            case "history_commit_detail":
              return Promise.resolve(args.hash === fx.mergeHash ? fx.detailMerge : fx.detail);
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
              return Promise.resolve(cfg.status);
            case "changes_detach":
            case "changes_reset":
            case "changes_create_branch":
            case "changes_revert":
            case "changes_cherry_pick":
              return act(cmd);
            default:
              return Promise.resolve(null);
          }
        },
      };
    },
    {
      repo: REPO,
      repos: REPOS,
      historyPage: HISTORY_PAGE,
      syncStatus: SYNC_STATUS,
      detail: COMMIT_DETAIL,
      detailMerge: COMMIT_DETAIL_MERGE,
      mergeHash: C3,
      diffText: DIFF_TEXT,
    },
    cfg
  );
  await page.goto(`${server.url}/history`, { waitUntil: "networkidle0" });
  await page.waitForFunction(() => document.body.innerText.includes("All branches"), { timeout: 5000 });
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
    ok("  and read the repo status", (await calls(page, "changes_repo_status")).length >= 1);
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
    ok("  stashing first, since the tree is dirty", r?.args.stashFirst === true);
    await page.close();
  }
  {
    const page = await open();
    await openPanel(page, C2);
    await openChooser(page);
    ok("Do it on the soft row", await clickInRow(page, "keep later changes staged", { text: "Do it" }));
    await page.waitForFunction(() => window.__CALLS__.some((c) => c.cmd === "changes_reset"), { timeout: 4000 });
    ok('  sends changes_reset mode "soft"', (await lastCall(page, "changes_reset"))?.args.mode === "soft");
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
  section("Go back: commits already on the upstream");
  {
    const page = await open({ resolved: RESOLVED_PUSHED });
    await openPanel(page, C2);
    await openChooser(page);
    const s = await text(page);
    ok("the amber note explains a force-push would be needed", s.includes("Moving main back would drop commits already on the upstream, which needs a force-push — GitSwitch never does that."));
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
    ok("  and the URL is still /history", (await page.evaluate(() => location.pathname)) === "/history");
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
    ok("Open in Changes goes to the Changes page for this repo", (await page.evaluate(() => location.pathname)) === "/changes" && (await page.evaluate(() => localStorage.getItem("gitswitch:changes.repo"))) === JSON.stringify(REPO));
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
