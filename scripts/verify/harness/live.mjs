// Live checks: the built Changes and History pages in headless Chrome, with
// every `invoke` forwarded to the REAL Rust backend, acting on a throwaway
// sandbox repository.
//
// ui.mjs and history.mjs prove the pages against a mocked backend; the shell
// suites prove the backend through `probe`. Neither would notice the two
// disagreeing — a wrong argument, a payload read under the wrong key, state the
// page forgets to re-read after an action. This harness closes that gap: the
// frontend's IPC bridge posts each call to this process, which runs the test
// binary's `invoke` op (src-tauri/src/probe.rs) — the same `#[tauri::command]`
// function the app would call, one process per call, under the sandbox HOME —
// and every page assertion is paired with a `git -C <sandbox>` check of the
// same fact.
//
// Two answers are shimmed because they depend on the machine, not on the repo:
// `history_list_repos` (the sandbox is in no profile's folders) and
// `get_profiles` (only when the real one fails). Tauri runtime calls
// (`plugin:*`) resolve to null — there is no Tauri here to answer them.
//
//   cd scripts/verify/harness && bun live.mjs      (LIVE_VERBOSE=1 logs every invoke)
import { launch, tally, DIST, DIST_OK } from "./browser.mjs";
import { spawn, spawnSync } from "node:child_process";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const VERBOSE = !!process.env.LIVE_VERBOSE;

if (!DIST_OK) {
  console.error("dist/ is missing — run `bun run build` first");
  process.exit(1);
}

const t = tally();
const { ok, section } = t;

// ---------------------------------------------------------------------------
// The sandbox: built by bash, HOME redirected first (scripts/verify/lib.sh).
// ---------------------------------------------------------------------------
console.log("building the fixture (lock helper, test binary, sandbox repos)…");
const fixture = spawnSync("bash", [path.join(HERE, "live-fixture.sh")], {
  encoding: "utf8",
  // The suites build with cargo; keep it off the network like the rest of all.sh's cargo steps.
  env: { ...process.env, CARGO_NET_OFFLINE: "true" },
  stdio: ["ignore", "pipe", "inherit"],
  maxBuffer: 64 * 1024 * 1024,
});
const envLine = (fixture.stdout ?? "").split("\n").find((l) => l.startsWith("LIVE_ENV "));
if (fixture.status !== 0 || !envLine) {
  console.error("could not build the fixture:\n" + (fixture.stdout ?? "").slice(-2000));
  process.exit(1);
}
/** {bin, sb, home, work, other, sub} — every path real (symlinks resolved), as the backend reports them. */
const ENV = JSON.parse(envLine.slice("LIVE_ENV ".length));
const WORK = ENV.work;
const OTHER = ENV.other;
const SUB = ENV.sub;
const REPO_REF = { path: WORK, name: "work", profile_id: "live", profile_name: "Live", profile_email: "live@example.test" };

/** Git under the sandbox HOME: the same identity and config the backend sees. */
const GIT_ENV = { ...process.env, HOME: ENV.home, GIT_CONFIG_NOSYSTEM: "1" };
function gitTry(repo, ...args) {
  const r = spawnSync("git", ["-C", repo, ...args], { env: GIT_ENV, encoding: "utf8" });
  // Only the trailing newline goes: porcelain output starts with a meaningful space (" M a.txt").
  return { ok: r.status === 0, out: (r.stdout ?? "").replace(/\n+$/, ""), err: (r.stderr ?? "").trim() };
}
function git(repo, ...args) {
  const r = gitTry(repo, ...args);
  if (!r.ok) throw new Error(`git -C ${repo} ${args.join(" ")} failed: ${r.err}`);
  return r.out;
}
const write = (file, content) => fs.writeFileSync(file, content);
const append = (file, content) => fs.appendFileSync(file, content);
const exists = (file) => fs.existsSync(file);

// ---------------------------------------------------------------------------
// The backend: one probe process per invoke, serialised like a UI thread would
// be by the index lock anyway.
// ---------------------------------------------------------------------------
function probeInvoke(cmd, args) {
  return new Promise((resolve) => {
    const started = Date.now();
    const child = spawn(ENV.bin, ["probe", "--ignored", "--nocapture"], {
      env: {
        ...process.env,
        HOME: ENV.home,
        GIT_CONFIG_NOSYSTEM: "1",
        PROBE_OP: "invoke",
        PROBE_CMD: cmd,
        PROBE_JSON: JSON.stringify(args ?? {}),
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    child.on("close", (code) => {
      const line = out.split("\n").find((l) => l.startsWith("PROBE_OUT "));
      let reply;
      if (!line) {
        reply = { ok: false, error: `probe printed no PROBE_OUT for ${cmd} (exit ${code}): ${(err || out).slice(-600)}` };
      } else {
        try {
          reply = JSON.parse(line.slice("PROBE_OUT ".length));
        } catch (e) {
          reply = { ok: false, error: `probe output for ${cmd} is not JSON: ${e}` };
        }
      }
      if (VERBOSE) {
        console.error(`  invoke ${cmd} ${JSON.stringify(args ?? {}).slice(0, 120)} → ${reply.ok ? "ok" : "ERR " + reply.error} (${Date.now() - started}ms)`);
      }
      resolve(reply);
    });
  });
}

let chain = Promise.resolve();
function serialised(fn) {
  const p = chain.then(fn, fn);
  chain = p.catch(() => {});
  return p;
}

/** Everything the page asks for. Two shims (see the header); the rest is the real backend. */
async function onInvoke(cmd, args) {
  if (cmd === "history_list_repos") return { ok: true, value: [REPO_REF] };
  if (cmd === "get_profiles") {
    const r = await serialised(() => probeInvoke(cmd, args));
    return r.ok ? r : { ok: true, value: [] };
  }
  return serialised(() => probeInvoke(cmd, args));
}

// ---------------------------------------------------------------------------
// ONE http server: dist/ with an SPA fallback, plus POST /invoke.
// ---------------------------------------------------------------------------
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".ico": "image/x-icon" };
function serveLive() {
  const server = http.createServer((req, res) => {
    if (req.method === "POST" && req.url === "/invoke") {
      let body = "";
      req.on("data", (d) => (body += d));
      req.on("end", async () => {
        let reply;
        try {
          const { cmd, args } = JSON.parse(body);
          reply = await onInvoke(cmd, args ?? {});
        } catch (e) {
          reply = { ok: false, error: `harness: ${e}` };
        }
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify(reply));
      });
      return;
    }
    const url = req.url.split("?")[0];
    let file = path.join(DIST, url === "/" ? "index.html" : url);
    if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) file = path.join(DIST, "index.html");
    res.writeHead(200, { "Content-Type": TYPES[path.extname(file)] ?? "text/plain" });
    res.end(fs.readFileSync(file));
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({ url: `http://127.0.0.1:${server.address().port}`, close: () => server.close() });
    });
  });
}

// ---------------------------------------------------------------------------
// Page helpers (the same hooks ui.mjs / history.mjs use: labels, titles, rows).
// ---------------------------------------------------------------------------
const text = (page) => page.evaluate(() => document.body.innerText);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const WAIT = 60000; // real git behind every click; generous so a slow disk is not a failure

// Chrome's innerText puts every flex item on its own line ("Up to date with" /
// "origin/main" / "· 1 to push" for one inline-flex span), so these two compare
// with whitespace collapsed — what the eye sees on the page.
const collapse = (s) => s.replace(/\s+/g, " ");
/** True once the page text contains `needle` (or false after the timeout — never throws). */
async function waitText(page, needle, timeout = WAIT) {
  try {
    await page.waitForFunction((n) => document.body.innerText.replace(/\s+/g, " ").includes(n), { timeout }, collapse(needle));
    return true;
  } catch {
    return false;
  }
}
/** True once the page text no longer contains `needle`. */
async function waitGone(page, needle, timeout = WAIT) {
  try {
    await page.waitForFunction((n) => !document.body.innerText.replace(/\s+/g, " ").includes(n), { timeout }, collapse(needle));
    return true;
  } catch {
    return false;
  }
}
/** Poll a synchronous predicate (usually a git fact) until true or the timeout. */
async function waitUntil(fn, timeout = WAIT) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try {
      if (fn()) return true;
    } catch {
      // keep polling
    }
    await sleep(150);
  }
  return false;
}

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
/** A button's disabled state and tooltip, by its visible label. */
async function buttonState(page, label) {
  const h = await button(page, label);
  if (!h) return null;
  return h.evaluate((b) => ({ disabled: b.disabled, title: b.title, text: b.innerText.trim() }));
}
/** Click `label`, failing the check (not the run) when the button is missing. */
async function click(page, label) {
  const h = await button(page, label);
  if (!h) return false;
  await h.click();
  return true;
}
/**
 * Click `label` once it is enabled. After an action the pages keep their
 * buttons disabled while they re-read the repository (real git takes a moment);
 * a click landing in that window would be a no-op.
 */
async function clickWhenEnabled(page, label, timeout = WAIT) {
  try {
    await page.waitForFunction(
      (l) => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === l && !b.disabled),
      { timeout },
      label
    );
  } catch {
    return false;
  }
  return click(page, label);
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
            ? buttons.find((x) => x.title === pick.title || x.title.startsWith(pick.title))
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
/** The open dialog titled `title` (Modal renders an <h2>), or null. */
const modalPanel = (title) => {
  const h = [...document.querySelectorAll("h2")].find((x) => x.innerText.trim() === title);
  return h ? h.parentElement.parentElement : null;
};
/** Click a button inside a row of the dialog titled `title` — the page behind it also has rows. */
async function clickInDialogRow(page, title, rowText, pick, timeout = WAIT) {
  // The dialog's buttons are disabled while the page is busy (a status re-read
  // right after navigating, say); clicking a disabled button silently does
  // nothing, so wait for an ENABLED match before clicking it.
  const finder = (title, rowText, pick, modalPanelSrc, doClick) => {
    const modalPanel = eval(modalPanelSrc);
    const panel = modalPanel(title);
    if (!panel) return false;
    // Prefer the row that NAMES rowText exactly (a leaf element whose text is
    // just that), then fall back to a substring match: a branch such as
    // "gitswitch-before-sync-…" whose row mentions "main" must not be picked
    // for "main".
    const all = [...panel.querySelectorAll("li")];
    const exact = all.filter((r) => [...r.querySelectorAll("*")].some((el) => el.children.length === 0 && el.innerText.trim() === rowText));
    const rows = exact.length ? exact : all.filter((r) => r.innerText.includes(rowText));
    for (const row of rows) {
      const buttons = [...row.querySelectorAll("button")];
      const b =
        pick.title !== undefined
          ? buttons.find((x) => x.title === pick.title)
          : buttons.find((x) => x.innerText.trim() === pick.text);
      if (b && !b.disabled) {
        if (doClick) b.click();
        return true;
      }
    }
    return false;
  };
  try {
    await page.waitForFunction(finder, { timeout }, title, rowText, pick, modalPanel.toString(), false);
  } catch {
    return false;
  }
  return page.evaluate(finder, title, rowText, pick, modalPanel.toString(), true);
}
/** The text and buttons of the first <li> mentioning `needle`. */
const rowWith = (page, needle) =>
  page.evaluate((n) => {
    const row = [...document.querySelectorAll("li")].find((r) => r.innerText.includes(n));
    if (!row) return null;
    return { text: row.innerText, buttons: [...row.querySelectorAll("button")].map((b) => ({ text: b.innerText.trim(), title: b.title, disabled: b.disabled })) };
  }, needle);
/**
 * Every row (text collapsed to one line) under the tree lists titled `title` ("Staged",
 * "Not staged", "Untracked", "Conflicts") — the parent's and any submodule
 * section's, since both carry the same headings.
 */
const listedUnder = (page, title) =>
  page.evaluate((title) => {
    const heads = [...document.querySelectorAll("h3")].filter((x) => x.innerText.trim() === title);
    if (heads.length === 0) return null;
    return heads.flatMap((h) => {
      const card = h.closest("div.rounded-xl");
      return card ? [...card.querySelectorAll("li")].map((li) => li.innerText.replace(/\s+/g, " ").trim()) : [];
    });
  }, title);
/** True once the list titled `title` has a row mentioning `needle`. */
async function waitListed(page, title, needle, timeout = WAIT) {
  try {
    await page.waitForFunction(
      (title, needle) =>
        [...document.querySelectorAll("h3")]
          .filter((x) => x.innerText.trim() === title)
          .some((h) => {
            const card = h.closest("div.rounded-xl");
            return !!card && [...card.querySelectorAll("li")].some((li) => li.innerText.includes(needle));
          }),
      { timeout },
      title,
      needle
    );
    return true;
  } catch {
    return false;
  }
}
/** Replace the content of an input/textarea (React-friendly: select, then type). */
async function typeInto(page, selector, value) {
  await page.evaluate((sel) => {
    const el = document.querySelector(sel);
    el.focus();
    el.select();
  }, selector);
  await page.keyboard.type(value);
}
const panelOpen = (page) => page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Copy hash"));

// ---------------------------------------------------------------------------
// Chrome, with the bridge installed before any app code runs.
// ---------------------------------------------------------------------------
const server = await serveLive();
const browser = await launch();
const page = await browser.newPage();
await page.setViewport({ width: 1280, height: 900 });
if (VERBOSE) {
  page.on("pageerror", (e) => console.error("  page error:", e.message));
  page.on("console", (m) => m.type() === "error" && console.error("  console:", m.text()));
}
await page.evaluateOnNewDocument((repo) => {
  window.__CALLS__ = [];
  // The remembered repository, as the app would have persisted it.
  localStorage.setItem("gitswitch:changes.repo", JSON.stringify(repo));
  localStorage.setItem("gitswitch:history.repo", JSON.stringify(repo));
  window.__TAURI_INTERNALS__ = {
    invoke: (cmd, args) => {
      window.__CALLS__.push({ cmd, args });
      if (cmd.startsWith("plugin:")) return Promise.resolve(null); // Tauri runtime, not a backend command
      return fetch("/invoke", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ cmd, args: args ?? {} }),
      })
        .then((r) => r.json())
        .then((j) => (j.ok ? j.value : Promise.reject(new Error(j.error))));
    },
  };
}, WORK);

async function openChanges() {
  await page.goto(`${server.url}/changes`, { waitUntil: "networkidle0", timeout: WAIT });
  if (!(await waitText(page, "Committing as"))) throw new Error("the Changes page never showed the repository: " + (await text(page)).slice(0, 400));
}
async function openHistory() {
  await page.goto(`${server.url}/history`, { waitUntil: "networkidle0", timeout: WAIT });
  await page.waitForFunction(() => /showing \d/.test(document.body.innerText), { timeout: WAIT });
}
/** "Go to commit" → the detail panel for `hash`. */
async function openPanel(hash) {
  await typeInto(page, 'input[aria-label="Go to commit"]', hash);
  await click(page, "Go");
  await page.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Copy hash"), { timeout: WAIT });
}
/** Open the Branches dialog and wait until it lists the branch `rowNeedle`. */
async function openBranches(rowNeedle) {
  await click(page, "Branches");
  await page.waitForFunction(
    (n, modalPanelSrc) => {
      const panel = eval(modalPanelSrc)("Branches");
      return !!panel && [...panel.querySelectorAll("li")].some((li) => li.innerText.includes(n) && li.querySelector("button"));
    },
    { timeout: WAIT },
    rowNeedle,
    modalPanel.toString()
  );
}

/** Run one scenario; an exception fails that scenario and the run goes on. */
async function scenario(title, fn) {
  section(title);
  try {
    await fn();
  } catch (e) {
    ok(`${title} — completed without an error`, false, String(e).slice(0, 600));
  }
}

const short = (oid) => oid.slice(0, 7);
const subjectOf = (repo, rev) => git(repo, "log", "-1", "--format=%s", rev);

try {
  // -----------------------------------------------------------------------
  await scenario("a. The submodule section acts on the real submodule", async () => {
    await openChanges();
    let s = await text(page);
    ok("the page shows the real branch and its upstream", s.includes("main") && s.includes("→ origin/main"));
    ok("the parent's gitlink row says the submodule has its own changes", s.includes("vendor/sub") && s.includes("has its own uncommitted changes"));
    ok("the ignored .env is not listed", !s.includes(".env"));
    const porcelain = git(WORK, "status", "--porcelain");
    ok("  git agrees: only the gitlink is dirty", porcelain.split("\n").length === 1 && porcelain.includes("vendor/sub"), porcelain);
    ok("an Inside vendor/sub section is present, on main, with one change", s.includes("Inside vendor/sub") && s.includes("on main") && s.includes("1 changed"));
    ok("  listing its dirty file", (await listedUnder(page, "Not staged"))?.some((p) => p.includes("file.txt")) === true);

    ok("Stage on the file inside the section", await clickInRow(page, "file.txt", { title: "Stage" }));
    ok("  stages it in the submodule", await waitUntil(() => git(SUB, "diff", "--cached", "--name-only") === "file.txt"));
    ok("  and not in the parent", git(WORK, "diff", "--cached", "--name-only") === "");
    ok("  the page says Staged.", await waitText(page, "Staged."));
    ok("  the section now lists it under Staged", await waitListed(page, "Staged", "file.txt"));

    await typeInto(page, 'section[id="submodule-section-vendor->sub"] textarea', "inside sub (live)");
    ok("Commit in vendor/sub is offered", await click(page, "Commit in vendor/sub"));
    ok("  the footer appears", await waitText(page, "Stage pointer"));
    ok("  git has the commit inside the submodule", subjectOf(SUB, "HEAD") === "inside sub (live)");
    const subHead = git(SUB, "rev-parse", "HEAD");
    s = await text(page);
    ok("  the footer names the new commit and says the parent still records the old one", s.includes(`Committed ${short(subHead)} inside vendor/sub`) && s.includes("still records the old commit"));
    ok("  the parent's gitlink row now says it points at a different commit", await waitText(page, "points at a different commit"));
    ok("  git agrees the parent records the old commit", git(WORK, "rev-parse", "HEAD:vendor/sub") !== subHead);

    ok("Stage pointer", await click(page, "Stage pointer"));
    ok("  stages the gitlink in the parent", await waitUntil(() => git(WORK, "diff", "--cached", "--name-only") === "vendor/sub"));
    ok("  and the page lists vendor/sub under Staged", await waitListed(page, "Staged", "vendor/sub"));

    await typeInto(page, "textarea", "bump sub (live)");
    ok("Commit in the parent", await click(page, "Commit"));
    ok("  records the pointer", await waitUntil(() => subjectOf(WORK, "HEAD") === "bump sub (live)"));
    ok("  at the submodule's HEAD", git(WORK, "rev-parse", "HEAD:vendor/sub") === subHead);
    ok("  the page reports the commit", await waitText(page, `Committed ${short(git(WORK, "rev-parse", "HEAD"))}.`));
    ok("  both trees are clean", git(WORK, "status", "--porcelain") === "" && git(SUB, "status", "--porcelain") === "");
    // Level everything with the sandbox remotes so later scenarios start from a known place.
    git(SUB, "push", "-q", "origin", "main");
    git(WORK, "push", "-q", "origin", "main");
  });

  // -----------------------------------------------------------------------
  await scenario("d. Undo last commit takes the commit apart", async () => {
    await openChanges();
    write(`${WORK}/d.txt`, "d\n");
    await click(page, "Refresh");
    ok("a new file appears under Untracked after Refresh", await waitListed(page, "Untracked", "d.txt"));
    ok("Stage on its row", await clickInRow(page, "d.txt", { title: "Stage" }));
    ok("  stages it", await waitUntil(() => git(WORK, "diff", "--cached", "--name-only") === "d.txt"));
    await waitText(page, "Staged.");
    await typeInto(page, "textarea", "add d (live)");
    ok("Commit", await click(page, "Commit"));
    ok("  git has the commit", await waitUntil(() => subjectOf(WORK, "HEAD") === "add d (live)"));
    const undoable = git(WORK, "rev-parse", "HEAD");
    ok("  the page reports it", await waitText(page, `Committed ${short(undoable)}.`));
    const st = await buttonState(page, "Undo last commit");
    ok("Undo last commit is offered for an unpushed commit", st?.disabled === false, JSON.stringify(st));
    ok("  Undo last commit", await click(page, "Undo last commit"));
    ok("  the page says Undid the last commit.", await waitText(page, "Undid the last commit."));
    ok("  the commit is gone", subjectOf(WORK, "HEAD") === "bump sub (live)" && git(WORK, "rev-parse", "HEAD") === git(WORK, "rev-parse", `${undoable}~1`));
    ok("  and its change is staged", git(WORK, "diff", "--cached", "--name-only") === "d.txt");
    const s = await text(page);
    ok("  the page lists d.txt under Staged", await waitListed(page, "Staged", "d.txt"));
    ok("  with the undo command", s.includes(`undo: git reset --soft ${undoable}`) || s.includes(`reset --soft ${undoable}`), s.split("\n").find((l) => l.startsWith("undo:")) ?? "(no undo line)");
    git(WORK, "reset", "-q", "--", "d.txt");
    fs.rmSync(`${WORK}/d.txt`);
  });

  // -----------------------------------------------------------------------
  await scenario("b. Stash changes… and Pop", async () => {
    append(`${WORK}/a.txt`, "wip\n");
    await openChanges();
    const st = await buttonState(page, "Stash changes…");
    ok("Stash changes… is offered on a dirty tree", st?.disabled === false, JSON.stringify(st));
    await click(page, "Stash changes…");
    await page.waitForSelector('input[aria-label="Stash message"]', { timeout: WAIT });
    ok("the dialog counts the change", (await text(page)).includes("1 uncommitted change (0 staged, 1 not staged, 0 new)"));
    await page.type('input[aria-label="Stash message"]', "wip live");
    ok("Stash", await click(page, "Stash"));
    ok("  the page says Stashed 1 change.", await waitText(page, "Stashed 1 change."));
    const list = git(WORK, "stash", "list").split("\n").filter(Boolean);
    ok("  git stash list has one entry", list.length === 1 && list[0].includes("wip live"), list.join(" | "));
    ok("  the tree is clean", git(WORK, "status", "--porcelain") === "");
    ok("  the Stashes card lists it", await waitText(page, "wip live"));
    const s = await text(page);
    ok("  with its count, ref and branch", s.includes("Stashes 1") && s.includes("stash@{0}") && s.includes("on main"));
    ok("Pop on the entry", await clickInRow(page, "wip live", { title: "Apply this stash and remove it" }));
    ok("  the page says Popped stash@{0}.", await waitText(page, "Popped stash@{0}."));
    ok("  the change is back", git(WORK, "status", "--porcelain") === " M a.txt", git(WORK, "status", "--porcelain"));
    ok("  and the stash is gone", git(WORK, "stash", "list") === "");
    ok("  the page lists a.txt under Not staged again", await waitListed(page, "Not staged", "a.txt"));
    ok("  and the Stashes card is gone", await waitGone(page, "Stashes 1"));
    git(WORK, "checkout", "-q", "--", "a.txt");
  });

  // -----------------------------------------------------------------------
  await scenario("c. Branches: create, switch, rename, delete", async () => {
    await openChanges();
    await click(page, "Branches");
    await page.waitForSelector('input[aria-label="New branch name"]', { timeout: WAIT });
    await page.type('input[aria-label="New branch name"]', "feature/live");
    ok("the switch checkbox is on by default", await page.$eval('input[aria-label="Switch to the new branch"]', (el) => el.checked));
    ok("Create branch", await click(page, "Create branch"));
    ok("  the page says Created and switched to feature/live.", await waitText(page, "Created and switched to feature/live."));
    ok("  git is on feature/live", git(WORK, "branch", "--show-current") === "feature/live");
    ok("  the dialog closed", await waitGone(page, "Create branch"));
    ok("  and the branch card shows the new branch, not published yet", (await text(page)).includes("feature/live") && (await text(page)).includes("not published yet"));

    await openBranches("main");
    ok("Switch is offered on main", await clickInDialogRow(page, "Branches", "main", { title: "Switch to this branch" }));
    ok("  the page says Switched to main.", await waitText(page, "Switched to main."));
    ok("  git is back on main", git(WORK, "branch", "--show-current") === "main");

    await openBranches("feature/live");
    ok("Rename… on feature/live", await clickInDialogRow(page, "Branches", "feature/live", { title: "Rename this branch" }));
    await page.waitForSelector('input[aria-label="Rename to"]', { timeout: WAIT });
    await typeInto(page, 'input[aria-label="Rename to"]', "feature/renamed");
    ok("  Save", await clickInDialogRow(page, "Branches", "feature/live", { text: "Save" }));
    ok("  the page says Renamed feature/live to feature/renamed.", await waitText(page, "Renamed feature/live to feature/renamed."));
    ok("  git lists the new name only", git(WORK, "branch", "--list", "feature/*").replace(/[* ]/g, "") === "feature/renamed");

    // A commit only that branch has, made behind the page's back.
    git(WORK, "switch", "-q", "feature/renamed");
    write(`${WORK}/f.txt`, "f\n");
    git(WORK, "add", "f.txt");
    git(WORK, "commit", "-qm", "on the feature (live)");
    git(WORK, "switch", "-q", "main");
    const featureTip = git(WORK, "rev-parse", "feature/renamed");
    await openBranches("feature/renamed");
    ok("Delete… on the unmerged branch", await clickInDialogRow(page, "Branches", "feature/renamed", { title: "Delete this branch" }));
    ok("  asks again", await waitText(page, "Delete feature/renamed?"));
    const s = await text(page);
    ok("  naming the commit it would drop", s.includes("1 commit"));
    ok("  and the branch still exists", git(WORK, "branch", "--list", "feature/renamed") !== "");
    ok("  Delete anyway", await click(page, "Delete anyway"));
    ok("  the page says Deleted feature/renamed.", await waitText(page, "Deleted feature/renamed."));
    ok("  git no longer lists it", git(WORK, "branch", "--list", "feature/renamed") === "");
    const undo = (await text(page)).split("\n").find((l) => l.startsWith("undo:")) ?? "";
    ok("  the result carries the undo command with the tip", undo.includes("branch feature/renamed") && undo.includes(featureTip), undo || "(no undo line)");
  });

  // -----------------------------------------------------------------------
  await scenario("e. Reset to origin/main… with a backup branch", async () => {
    append(`${WORK}/a.txt`, "local\n");
    git(WORK, "commit", "-qam", "local only (live)");
    const oldTip = git(WORK, "rev-parse", "HEAD");
    await openChanges();
    const st = await buttonState(page, "Reset to origin/main…");
    ok("Reset to origin/main… is offered one commit ahead", st?.disabled === false, JSON.stringify(st));
    await click(page, "Reset to origin/main…");
    ok("  it asks first", await waitText(page, "Reset to origin/main?"));
    ok("  counting the unpushed commit", (await text(page)).includes("Your 1 unpushed commit leaves the branch."));
    ok("  a clean tree offers no stash choice", (await page.$('input[aria-label="Stash uncommitted changes first"]')) === null);
    ok("  Reset", await click(page, "Reset"));
    ok("  the result shows the undo command", await waitText(page, "undo: git -C"));
    ok("  HEAD equals origin/main", git(WORK, "rev-parse", "HEAD") === git(WORK, "rev-parse", "origin/main"));
    const backups = git(WORK, "branch", "--list", "gitswitch-before-reset-*").split("\n").map((b) => b.replace(/[* ]/g, "")).filter(Boolean);
    ok("  a gitswitch-before-reset-* branch exists", backups.length === 1, backups.join(","));
    ok("  holding the old tip", backups.length === 1 && git(WORK, "rev-parse", backups[0]) === oldTip);
    const s = await text(page);
    ok("  the result card names the backup branch and the commit that left", backups.length === 1 && s.includes(backups[0]) && s.includes("local only (live)"));
    ok("  the page is level with origin/main again", s.includes("Nothing to pull") && (await buttonState(page, "Reset to origin/main…"))?.disabled === true);
  });

  // -----------------------------------------------------------------------
  await scenario("f. Pull with rebase on a dirty tree needs autostash", async () => {
    git(OTHER, "pull", "-q", "--ff-only");
    write(`${OTHER}/up.txt`, "up\n");
    git(OTHER, "add", "-A");
    git(OTHER, "commit", "-qm", "upstream work (live)");
    git(OTHER, "push", "-q", "origin", "main");
    append(`${WORK}/a.txt`, "dirt\n");
    await openChanges();
    ok("Fetch", await click(page, "Fetch"));
    ok("  the page says Fetched. with the count", await waitText(page, "1 commit(s) waiting to be pulled."));
    ok("  git agrees origin/main moved", git(WORK, "rev-parse", "origin/main") === git(OTHER, "rev-parse", "HEAD"));
    ok("  the fast-forward sentence names the incoming commit", (await text(page)).includes("picking up 1 new commit from origin/main"));
    ok("Rebase", await click(page, "Rebase"));
    let st = await buttonState(page, "Pull");
    ok("  on a dirty tree Pull (rebase) is disabled", st?.text === "Pull (rebase)" && st.disabled === true, JSON.stringify(st));
    ok("  saying why", (st?.title ?? "").includes("clean working tree") && (await text(page)).includes("turn on autostash below"));
    const box = await page.$('input[aria-label="Stash changes around the rebase"]');
    ok("  autostash is offered, off", box !== null && !(await box.evaluate((el) => el.checked)));
    await box.evaluate((el) => el.click());
    st = await buttonState(page, "Pull");
    ok("  with autostash on, Pull is enabled", st?.disabled === false, JSON.stringify(st));
    ok("Pull (rebase)", await click(page, "Pull"));
    ok("  the page says Pulled 1 commit(s) with rebase", await waitText(page, "Pulled 1 commit(s) with rebase"));
    ok("  git: HEAD is the upstream commit", subjectOf(WORK, "HEAD") === "upstream work (live)" && git(WORK, "rev-parse", "HEAD") === git(WORK, "rev-parse", "origin/main"));
    ok("  the dirty change is back", git(WORK, "status", "--porcelain") === " M a.txt", git(WORK, "status", "--porcelain"));
    ok("  no autostash entry is left", git(WORK, "stash", "list") === "");
    ok("  the page is level again and still lists a.txt", await waitText(page, "Nothing to pull") && (await listedUnder(page, "Not staged"))?.some((p) => p.includes("a.txt")) === true);
    git(WORK, "checkout", "-q", "--", "a.txt");
  });

  // -----------------------------------------------------------------------
  await scenario("g. A merge conflict: Take theirs, then the commit concludes the merge", async () => {
    git(OTHER, "pull", "-q", "--ff-only");
    write(`${OTHER}/c.txt`, "theirs\n");
    git(OTHER, "commit", "-qam", "theirs c (live)");
    git(OTHER, "push", "-q", "origin", "main");
    write(`${WORK}/c.txt`, "mine\n");
    git(WORK, "commit", "-qam", "mine c (live)");
    git(WORK, "fetch", "-q");
    gitTry(WORK, "merge", "origin/main"); // exits 1: the conflict is the point
    ok("git is mid-merge with c.txt in conflict", exists(`${WORK}/.git/MERGE_HEAD`) && git(WORK, "diff", "--name-only", "--diff-filter=U") === "c.txt");
    await openChanges();
    let s = await text(page);
    ok("the page shows the merge in progress", /merge in progress/i.test(s));
    ok("  with a Conflicts list naming c.txt as both modified", s.includes("Conflicts") && s.includes("c.txt") && s.includes("both modified"));
    ok("  saying what mine means", s.includes("mine = main"));
    const row = await rowWith(page, "c.txt");
    ok("  the row offers Keep mine and Take theirs", row !== null && row.buttons.some((b) => b.text === "Keep mine") && row.buttons.some((b) => b.text === "Take theirs"), JSON.stringify(row?.buttons));
    ok("  and Commit is blocked while the conflict remains", (await buttonState(page, "Commit"))?.disabled === true);
    ok("Take theirs", await clickInRow(page, "c.txt", { text: "Take theirs" }));
    ok("  the page reports the file resolved", await waitText(page, "1 file(s) resolved."));
    ok("  c.txt holds their content", fs.readFileSync(`${WORK}/c.txt`, "utf8") === "theirs\n");
    ok("  and is staged, no conflict left", git(WORK, "diff", "--cached", "--name-only") === "c.txt" && git(WORK, "diff", "--name-only", "--diff-filter=U") === "");
    s = await text(page);
    ok("  the commit box says this commit finishes the merge", s.includes("finishes the merge"));
    const shown = await page.$eval("textarea", (el) => el.value);
    ok("  with the merge message prefilled", shown.startsWith("Merge remote-tracking branch 'origin/main'"), shown.slice(0, 80));
    const commit = await buttonState(page, "Commit");
    ok("  and Commit enabled", commit?.disabled === false, JSON.stringify(commit));
    ok("Commit", await click(page, "Commit"));
    const settled = await Promise.race([waitText(page, "Committed ", 15000), waitText(page, "A commit needs a message", 15000)]);
    s = await text(page);
    ok("  the merge is concluded (no MERGE_HEAD)", settled && !exists(`${WORK}/.git/MERGE_HEAD`), s.includes("A commit needs a message") ? "the page sent an empty message: 'A commit needs a message.'" : "");
    ok("  git: HEAD is a merge commit with the right subject", git(WORK, "rev-list", "--parents", "-1", "HEAD").split(" ").length === 3 && subjectOf(WORK, "HEAD") === "Merge remote-tracking branch 'origin/main'");
    ok("  the tree is clean", git(WORK, "status", "--porcelain") === "");
    ok("  and the page no longer shows the merge banner or conflicts", await waitGone(page, "finishes the merge") && !/merge in progress/i.test(await text(page)));
    if (exists(`${WORK}/.git/MERGE_HEAD`)) {
      // Leave the sandbox usable for the remaining scenarios.
      git(WORK, "commit", "-qm", "Merge remote-tracking branch 'origin/main'");
    }
    git(WORK, "push", "-q", "origin", "main");
  });

  // -----------------------------------------------------------------------
  await scenario("j. Sync card: Assess (fetches) then Sync now", async () => {
    git(OTHER, "pull", "-q", "--ff-only");
    write(`${OTHER}/more.txt`, "more\n");
    git(OTHER, "add", "-A");
    git(OTHER, "commit", "-qm", "other 2 (live)");
    git(OTHER, "push", "-q", "origin", "main");
    const incoming = git(OTHER, "rev-parse", "HEAD");
    await openChanges();
    ok("Sync now is not offered before a plan exists", (await button(page, "Sync now")) === null);
    ok("Assess (fetches)", await click(page, "Assess (fetches)"));
    ok("  the plan is a sentence", await waitText(page, "replayed on top of 1 new commit from origin/main"));
    let s = await text(page);
    ok("  main is 0 ahead and 1 behind", s.includes("main is 0 ahead and 1 behind origin/main"));
    ok("  the submodule table says vendor/sub has nothing to do", s.includes("vendor/sub") && s.includes("nothing to do"));
    ok("  Assess fetched: git sees the incoming commit", git(WORK, "rev-parse", "origin/main") === incoming);
    ok("  nothing moved yet", git(WORK, "rev-parse", "HEAD") !== incoming);
    const st = await buttonState(page, "Sync now");
    ok("  Sync now is enabled", st?.disabled === false, JSON.stringify(st));
    ok("Sync now", await click(page, "Sync now"));
    ok("  the page says Synced: fast-forwarded", await waitText(page, "Synced: fast-forwarded to 1 new commit from origin/main."));
    ok("  git: HEAD equals origin/main at other's commit", git(WORK, "rev-parse", "HEAD") === incoming && subjectOf(WORK, "HEAD") === "other 2 (live)");
    ok("  the tree is clean and no stash is left", git(WORK, "status", "--porcelain") === "" && git(WORK, "stash", "list") === "");
    s = await text(page);
    ok("  the result table shows the new HEAD", s.includes(`→ ${short(incoming)}`));
    ok("  every check passed", s.includes("All 6 checks passed"));
    ok("  the page reflects the new HEAD: level with origin/main, plan cleared", s.includes("Nothing to pull — you're level with origin/main") && (await button(page, "Assess (fetches)")) !== null);
  });

  // -----------------------------------------------------------------------
  await scenario("h. History: Go to commit, look at it (detached), back to main from Changes", async () => {
    const tip = git(WORK, "rev-parse", "HEAD");
    const target = git(WORK, "rev-parse", "HEAD~1");
    await openHistory();
    ok("the History page lists the real commits", (await text(page)).includes("other 2 (live)"));
    ok("  and says it is up to date with origin/main", await waitText(page, "Up to date with origin/main"));
    await openPanel(target);
    let s = await text(page);
    ok("Go to commit opens the panel with the real subject and hash", s.includes(subjectOf(WORK, target)) && s.includes(target));
    await click(page, "Go back to this commit…");
    await page.waitForSelector('[role="radiogroup"][aria-label="Go back mode"]', { timeout: WAIT });
    await waitGone(page, "Checking where this commit sits");
    s = await text(page);
    ok("  the chooser knows the commits after it are on the upstream", s.includes("would drop commits already on the upstream"));
    const look = await rowWith(page, "Look at it");
    ok("  Look at it stays available", look !== null && look.buttons.some((b) => b.text === "Do it" && !b.disabled));
    ok("  Do it", await clickInRow(page, "Look at it", { text: "Do it" }));
    ok("  the page says Looking at <short>", await waitText(page, `Looking at ${short(target)}`));
    ok("  git: HEAD is detached at that commit", git(WORK, "rev-parse", "HEAD") === target && !gitTry(WORK, "symbolic-ref", "-q", "HEAD").ok);
    ok("  the panel closed", !(await panelOpen(page)));
    ok("Open in Changes (once the page has re-read the repository)", await clickWhenEnabled(page, "Open in Changes"));
    ok("  Changes says detached HEAD at the commit", await waitText(page, `detached HEAD at ${short(target)}`));
    await openBranches("main");
    ok("  Switch on main", await clickInDialogRow(page, "Branches", "main", { title: "Switch to this branch" }));
    const switched = await waitText(page, "Switched to main.");
    if (!switched) {
      console.log("DEBUG after Switch click — git HEAD:", gitTry(WORK, "symbolic-ref", "-q", "HEAD").out, "| page:", (await text(page)).replace(/\s+/g, " ").slice(0, 1500));
    }
    ok("  the page says Switched to main.", switched);
    ok("  git is on main at the tip again", git(WORK, "branch", "--show-current") === "main" && git(WORK, "rev-parse", "HEAD") === tip);
  });

  // -----------------------------------------------------------------------
  await scenario("i. History: Undo this commit (revert) makes a Revert commit", async () => {
    gitTry(WORK, "switch", "-q", "main"); // a failed h would leave HEAD detached
    const head = git(WORK, "rev-parse", "HEAD");
    ok("the tip is a plain commit on main", git(WORK, "branch", "--show-current") === "main" && git(WORK, "rev-list", "--parents", "-1", "HEAD").split(" ").length === 2);
    await openHistory();
    await openPanel(head);
    await click(page, "Undo this commit (revert)");
    ok(`it asks "Revert ${short(head)}?"`, await waitText(page, `Revert ${short(head)}?`));
    ok("  with no mainline question for a plain commit", (await page.$('[role="radiogroup"][aria-label="Mainline"]')) === null);
    ok("  Revert", await click(page, "Revert"));
    ok("  the page says Reverted <short>.", await waitText(page, `Reverted ${short(head)}.`));
    ok("  git: a Revert commit is on main", subjectOf(WORK, "HEAD") === 'Revert "other 2 (live)"' && git(WORK, "branch", "--show-current") === "main");
    ok("  the reverted file is gone", !exists(`${WORK}/more.txt`));
    ok("  the branch is 1 ahead of upstream", git(WORK, "rev-list", "--count", "origin/main..HEAD") === "1");
    ok("  the History page lists the revert", await waitText(page, 'Revert "other 2 (live)"'));
    ok("  and says 1 to push (after the sync status was re-read)", await waitText(page, "Up to date with origin/main · 1 to push"));
  });
} finally {
  await browser.close();
  server.close();
}

process.exit(t.done() ? 0 : 1);
