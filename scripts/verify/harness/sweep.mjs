// Every page at half-screen widths, measuring the element that actually
// scrolls. `<main>` has overflow-y-auto, so it scrolls horizontally on its own
// and documentElement never reports overflow — measuring the wrong element once
// hid a page that was visibly clipped.
import fs from "node:fs";
import { launch, serve, DIST_OK } from "./browser.mjs";
import { REPOS, SCENARIOS, DIFF_TEXT, SUB_STATUSES, STASHES, SUBMODULES } from "./fixtures.mjs";

if (!DIST_OK) {
  console.error("dist/ is missing — run `npm run build` first");
  process.exit(1);
}

const status = {
  ...SCENARIOS.dirty,
  name: "argos",
  entries: [
    { ...SCENARIOS.dirty.entries[2], path: "packages/worker/src/infrastructure/persistence/repositories/very-long-name.ts" },
    { ...SCENARIOS.dirty.entries[2], path: ".seed/gate-receipts/local-e297545bea5310a95a353cc6867072e3cfab5cca.json", kind: "untracked", unstaged: "?" },
    // A gitlink with a long path: the row gets a submodule reason and no line counts.
    { ...SCENARIOS.dirty.entries[2], path: "vendor/forge-tooling/aggregation/testing-harness/fixtures/very-long-submodule-name", is_submodule: true, sub_commit_changed: true, sub_tracked_changes: true, unstaged_added: null, unstaged_removed: null },
  ],
  staged_count: 0,
  unstaged_count: 2,
  untracked_count: 1,
  has_submodules: true,
  stash_count: 2,
};
const PAGES = ["Profiles", "SSH Keys", "GitHub Auth", "Repositories", "Clone", "Repository", "Ports", "Auto Assign", "Doctor", "Commit Audit", "Settings"];

// The app's own content-security policy, served with the page: anything the
// built app does that the policy forbids shows up here as a violation.
const security = JSON.parse(fs.readFileSync(new URL("../../../src-tauri/tauri.conf.json", import.meta.url), "utf8")).app?.security ?? {};
const csp = typeof security.csp === "string" ? security.csp : security.csp ? Object.entries(security.csp).map(([k, v]) => `${k} ${Array.isArray(v) ? v.join(" ") : v}`).join("; ") : null;
if (!csp) {
  console.error("the app declares no content-security policy");
  process.exit(1);
}
const server = await serve({ headers: { "Content-Security-Policy": csp } });
const browser = await launch();
let bad = 0;
/** Violations recorded since the last look, reported and counted. */
const cspCheck = async (page, where) => {
  const v = await page.evaluate(() => (window.__CSP__ || []).splice(0));
  if (v.length) {
    bad++;
    console.log(`  \x1b[31mCSP     \x1b[0m ${where.padEnd(14)} ${[...new Set(v)].join(" | ")}`);
  }
};
try {
  for (const width of [700, 900]) {
    console.log(`\n\x1b[1m${width}px (half screen)\x1b[0m`);
    const page = await browser.newPage();
    await page.setViewport({ width, height: 900 });
    await page.evaluateOnNewDocument(() => {
      document.addEventListener("securitypolicyviolation", (e) => (window.__CSP__ = window.__CSP__ || []).push(`${e.violatedDirective} ${e.blockedURI}`));
    });
    await page.evaluateOnNewDocument((repos, st, d1, subs, stashes, submods) => {
      localStorage.clear();
      // The Clone page takes its readings only once it has a URL and a folder.
      localStorage.setItem("gitswitch:sparse.url", JSON.stringify("git@github.com:me/big-data.git"));
      localStorage.setItem("gitswitch:sparse.parentDir", JSON.stringify("/repos"));
      localStorage.setItem("gitswitch:changes.repo", JSON.stringify("/repos/gitswitch"));
      localStorage.setItem("gitswitch:history.repo", JSON.stringify("/repos/gitswitch"));
      window.__LONG_KEY__ = "/Users/me/.ssh/id_ed25519_gitswitch_acme_platform_infrastructure_and_release_engineering";
      window.__LONG_EMAIL__ = "releaseengineeringautomationandplatforminfrastructureteamnotifications@corpmail.example.test";
      const profiles = [
        { id: "p1", name: "Kirmada", git_name: "Me", git_email: "me@personal.test", ssh_key_path: "/Users/me/.ssh/id_ed25519_personal", is_default: true, allow_push: true, signing_enabled: false, directories: ["/Users/me/Documents/Acme/gitswitch", "/Users/me/Documents/very/long/folder/path/that/goes/on"], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
        { id: "p2", name: "Work", git_name: "Me At Work", git_email: "me@work.test", ssh_key_path: null, is_default: false, allow_push: false, signing_enabled: true, directories: ["/Users/me/Documents/Work"], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
        // An address with no break opportunity: letters, dots and @ give none.
        { id: "p3", name: "Long", git_name: "Long Address", git_email: window.__LONG_EMAIL__, ssh_key_path: null, is_default: false, allow_push: true, signing_enabled: false, directories: [], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
      ];
      const audit = [{
        path: "/repos/argos", repo_name: "argos", profile_name: "Long", expected_name: "Long Address", expected_email: window.__LONG_EMAIL__,
        mismatched: [{ hash: "a".repeat(40), short: "aaaaaaa", author_name: "Someone", author_email: window.__LONG_EMAIL__, date: "2026-01-01T00:00:00Z", subject: "a commit", pushed: false, branches: ["main"], on_head: true }],
        unpushed_count: 1, pushed_count: 0, has_upstream: true, dirty: false, guard: "none", other_branch_count: 0, fix_blocker: null,
      }];
      // Auto Assign's scan, only once a test turns it on (window.__AUTO__): the
      // page loop's Refresh-parity readings stay what they were.
      const auto = {
        scan_repos: [{ path: "/repos/argos-platform-infrastructure", remote_url: "git@github.com:acme/argos.git", owner: "acme", name: "argos" }],
        resolve_key_account: "me", gh_list_accounts: ["me"], gh_get_token: "tok",
        verify_github_token: { id: 1, login: "me", name: null, email: null, avatar_url: "" },
        check_repo_access: { admin: false, push: true, pull: true },
      };
      window.__TAURI_INTERNALS__ = {
        // The terminal's output stream is a Channel, and its constructor calls
        // this. Without it the drawer throws, renders an error instead of a
        // shell, and any overflow check below would measure the wrong thing.
        transformCallback: (callback) => {
          const id = (window.__NEXT_CB__ = (window.__NEXT_CB__ || 0) + 1);
          (window.__CHANNELS__ = window.__CHANNELS__ || {})[id] = callback;
          return id;
        },
        invoke: (cmd) => {
          // Every call, in order: what a page asks when it opens must be asked again by its Refresh.
          (window.__CALLS__ = window.__CALLS__ || []).push(cmd);
          if (window.__AUTO__ && cmd in auto) return Promise.resolve(auto[cmd]);
          switch (cmd) {
            case "audit_commits": return Promise.resolve(audit);
            case "doctor_check_profiles": return Promise.resolve([{ id: "folder-missing", severity: "warning", title: "Folder assigned to \"Long\" doesn't exist", detail: "/Users/me/Documents/releaseengineeringautomationandplatforminfrastructureteamnotifications/workspace is not a directory.", fix: null, fix_label: null }]);
            case "get_profiles": return Promise.resolve(profiles);
            case "history_list_repos": return Promise.resolve(repos);
            case "changes_repo_status": return Promise.resolve(st);
            case "history_head": return Promise.resolve({ branch: st.branch, detached: st.detached, unborn: st.unborn });
            case "history_authors": return Promise.resolve([{ name: "Me", email: "me@example.test", commits: 2 }, { name: "Other", email: "o@example.test", commits: 1 }]);
            case "clone_progress": return Promise.resolve(null);
            case "clone_size_estimate": return Promise.resolve({ repo: "owner/repo", kb: 1258291 });
            case "changes_push_identity": return Promise.resolve({ remote: "origin", url: "git@github.com:octo-work/gitswitch.git", via: "ssh-key", login: "octo-work", key_path: "/Users/me/.ssh/id_work", host: "github.com", account: null, expected_login: null, detail: "github.com accepted the key id_work." });
            case "changes_submodule_statuses": return Promise.resolve(subs);
            case "changes_submodules": return Promise.resolve(submods);
            case "changes_stash_list": return Promise.resolve(stashes);
            case "changes_file_diff": return Promise.resolve(d1);
            case "changes_lfs_progress": return Promise.resolve(null);
            case "terminal_open": return Promise.resolve("sweep-session");
            case "terminal_write": case "terminal_resize": case "terminal_close": return Promise.resolve(null);
            case "console_entries": return Promise.resolve([]);
            case "changes_push_state": return Promise.resolve({ ...st.push, blocked: false, repo_blocked: false, lock: { ...st.push.lock, locked: false }, reason: "Pushing is allowed from this folder." });
            case "changes_lfs_files": return Promise.resolve({ installed: true, version: "git-lfs/3.8.0", uses_lfs: false, filters_configured: true, files: [], folders: [], total: 0, present: 0, missing: 0, total_bytes: 0, missing_bytes: 0, sizes_known: true, truncated: 0, folders_truncated: 0, summary: "This repository doesn't use Git LFS." });
            case "get_current_git_config": return Promise.resolve("[user]\n\tname = Me\n\temail = me@personal.test");
            case "allowed_signers_path": return Promise.resolve("/Users/me/.ssh/allowed_signers");
            case "local_clone_index": return Promise.resolve({ by_path: {}, by_repo: {} });
            case "clone_destination_status": return Promise.resolve({ path: "/repos/big-data", exists: false, is_repo: false, origin: null, same_repo: false, same_url: false, origin_is_https: false, incomplete: false });
            case "list_remote_repos": return Promise.resolve({ repos: [], sso_hidden_orgs: 0, truncated: false });
            case "list_remote_repos_page": return Promise.resolve({ listing: { account: "a", login: "me", suggested_profile_id: null, repos: [], sso_hidden_orgs: 0, truncated: false }, page: 1, per_page: 10, has_more: false });
            case "history_branches": return Promise.resolve([{ name: "main", is_current: true, is_remote: false, upstream: "origin/main", ahead: 0, behind: 0, tip: "aaa", short_tip: "aaa", last_author: "Me", last_date: "2026-01-01T00:00:00Z", subject: "x" }]);
            case "history_page": return Promise.resolve({ commits: [], total: 0, has_more: false });
            // The Ports page: a long folder and a long container name, so the sweep measures the worst row.
            case "ports_list": return Promise.resolve({ os: "macos", docker: "ok", took_ms: 120, notes: ["Processes of other users (including root) are listed from netstat; GitSwitch cannot inspect their folder or stop them."], rows: [
              { key: "tcp:18789:725", proto: "tcp", port: 18789, addresses: ["127.0.0.1", "::1"], scope: "loopback", pid: 725, name: "node", exe: "/opt/homebrew/bin/node", cwd: "/Users/me/Documents/releaseengineeringautomationandplatforminfrastructureteamnotifications/workspace/apps/web", started_at: "2026-10-11T01:00:00+05:30", user: "me", ours: true, workspace: "abc12345", clone: { path: "/Users/me/Documents/Acme/gitswitch", name: "gitswitch", profile_name: "Kirmada" }, container: null, killable: true, why_not: null },
              { key: "tcp:8080:2842", proto: "tcp", port: 8080, addresses: ["*"], scope: "all", pid: 2842, name: "com.docker.backend", exe: null, cwd: null, started_at: null, user: "me", ours: false, workspace: null, clone: null, container: { id: "f50b929f94ba", name: "releaseengineeringautomationandplatforminfrastructure-proxy-1", project: "releaseengineeringautomationandplatforminfrastructure", service: "proxy", working_dir: "/Users/me/x", container_port: 80 }, killable: false, why_not: "Published by Docker — stop the container instead." },
              { key: "tcp:8021:1", proto: "tcp", port: 8021, addresses: ["127.0.0.1", "::1"], scope: "loopback", pid: 1, name: "launchd", exe: "/sbin/launchd", cwd: null, started_at: null, user: "root", ours: false, workspace: null, clone: null, container: null, killable: false, why_not: "A system process (pid 1) — not stoppable from here." },
            ] });
            // The SSH Key Wizard's Test step, failing on a key whose path has no
            // break opportunity (underscores only: a hyphen would hide the bug).
            case "generate_ssh_key": return Promise.resolve([window.__LONG_KEY__, window.__LONG_KEY__ + ".pub"]);
            case "get_public_key": return Promise.resolve("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGitSwitchSweepKeyMaterialThatIsLongEnoughToWrap me@personal.test");
            case "test_connection_with_key": return Promise.reject("SSH error: Permission denied for key " + window.__LONG_KEY__);
            case "resolve_key_account": return Promise.resolve(null);
            default: return Promise.resolve([]);
          }
        },
      };
    }, REPOS, status, DIFF_TEXT, SUB_STATUSES, STASHES, SUBMODULES);
    // Start off a page that is not swept, so the first stop (Profiles, at "/") mounts fresh on its click.
    await page.goto(`${server.url}/profile/new`, { waitUntil: "networkidle0" });
    // The check itself must see a violation when there is one: an inline script is one.
    if (width === 700) {
      await page.evaluate(() => {
        const s = document.createElement("script");
        s.textContent = "window.__INLINE_RAN__ = true";
        document.body.appendChild(s);
      });
      await new Promise((r) => setTimeout(r, 200));
      const seen = await page.evaluate(() => ({ v: (window.__CSP__ || []).splice(0), ran: window.__INLINE_RAN__ === true }));
      const works = !seen.ran && seen.v.some((x) => x.startsWith("script-src"));
      if (!works) bad++;
      console.log(`  ${works ? "\x1b[32mok      \x1b[0m" : "\x1b[31mCSP     \x1b[0m"} ${"CSP check".padEnd(14)} ${works ? "the policy is served, and an inline script is blocked and reported" : `the policy did not block an inline script (${JSON.stringify(seen)})`}`);
    }
    await cspCheck(page, "first load");

    // The Repository page's other tabs are swept as pages of their own.
    const STOPS = PAGES.flatMap((n) => (n === "Repository" ? [n, "tab:History", "tab:Submodules"] : [n]));
    for (const stop of STOPS) {
      const name = stop.replace(/^tab:/, "");
      // Counted before the click: React runs a page's first effects inside the
      // click that opened it, so its first readings are already on the list.
      const callsBefore = await page.evaluate(() => (window.__CALLS__ || []).length);
      const clicked = await page.evaluate((n, isTab) => {
        const a = [...document.querySelectorAll(isTab ? 'a[role="tab"]' : "nav a")].find((x) => x.innerText.trim().startsWith(n));
        if (a) { a.click(); return true; }
        return false;
      }, name, stop.startsWith("tab:"));
      if (!clicked) { console.log(`  ${name}: link not found`); bad++; continue; }
      await new Promise((r) => setTimeout(r, 450));
      await cspCheck(page, name);
      const r = await page.evaluate(() => {
        const m = document.querySelector("main");
        const culprits = [];
        for (const el of document.querySelectorAll("main *")) {
          if (el.scrollWidth > el.clientWidth + 1 && el.clientWidth > 0) {
            const deeper = [...el.querySelectorAll("*")].some((c) => c.scrollWidth > c.clientWidth + 1 && c.clientWidth > 0);
            const cs = getComputedStyle(el);
            if (!deeper && cs.overflowX !== "auto" && cs.overflowX !== "scroll" && cs.textOverflow !== "ellipsis")
              culprits.push(`<${el.tagName.toLowerCase()} class="${(el.className || "").toString().slice(0, 50)}"> "${(el.innerText || "").slice(0, 28).replace(/\n/g, " ")}"`);
          }
        }
        return { need: m.scrollWidth, have: m.clientWidth, culprits: culprits.slice(0, 3) };
      });
      const over = r.need > r.have + 1;
      if (over) bad++;
      if (over && process.env.SWEEP_VERBOSE) {
        // Which elements actually reach past main's right edge (the culprit
        // list above names the deepest overflowing box, which can be a bystander).
        const wide = await page.evaluate(() => {
          const m = document.querySelector("main");
          const right = m.getBoundingClientRect().right;
          const out = [];
          for (const el of m.querySelectorAll("*")) {
            const r = el.getBoundingClientRect();
            if (r.width > 0 && r.right > right + 1 && ![...el.children].some((c) => c.getBoundingClientRect().right > right + 1))
              out.push(`${Math.round(r.right - right)}px past: <${el.tagName.toLowerCase()} class="${(el.className || "").toString().slice(0, 70)}"> "${(el.innerText || "").slice(0, 40).replace(/\n/g, " ")}"`);
          }
          return out.slice(0, 12);
        });
        for (const w of wide) console.log(`             ${w}`);
      }
      console.log(`  ${over ? "\x1b[31mOVERFLOW\x1b[0m" : "\x1b[32mok      \x1b[0m"} ${name.padEnd(14)} needs ${String(r.need).padStart(4)} has ${r.have}`);
      for (const c of r.culprits) console.log(`             ${c}`);
      // Every page re-reads on demand, under the same name.
      if (!stop.startsWith("tab:")) {
        const hasRefresh = await page.evaluate(() => [...document.querySelectorAll("button")].some((b) => b.innerText.trim() === "Refresh"));
        if (!hasRefresh) {
          bad++;
          console.log(`             no Refresh button on ${name}`);
        } else {
          // Refresh must do what a fresh visit does: everything the page asked
          // the backend when it opened, it asks again — not one reading of several.
          // (A page that polls while something runs is not running anything here.)
          // The Clone page asks GitHub about the size 600 ms after it opens.
          await new Promise((r) => setTimeout(r, 500));
          const opened = await page.evaluate((from) => (window.__CALLS__ || []).slice(from), callsBefore);
          const asked = [...new Set(opened)];
          const mark = await page.evaluate(() => (window.__CALLS__ || []).length);
          await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Refresh").click());
          await new Promise((r) => setTimeout(r, 1500));
          const again = await page.evaluate((from) => (window.__CALLS__ || []).slice(from), mark);
          const missing = asked.filter((c) => !again.includes(c));
          if (missing.length) {
            bad++;
            console.log(`             Refresh on ${name} did not re-ask: ${missing.join(", ")} (opened with: ${asked.join(", ")})`);
          } else if (asked.length) {
            console.log(`             Refresh re-asks all ${asked.length}: ${asked.join(", ")}`);
          } else if (name !== "Auto Assign") {
            // Only Auto Assign reads nothing on open (it scans a chosen folder).
            bad++;
            console.log(`             ${name} asked nothing on open — Refresh parity not checked`);
          }
        }
      }
    }

    // The terminal drawer is `fixed`, so it sits outside `main` and can never
    // inflate main.scrollWidth — it has to be measured on its own. Its header
    // carries a w-40 and a w-64 Select, which is the real risk at 700px.
    const opened = await page.evaluate(() => {
      const b = [...document.querySelectorAll("button")].find((x) => x.innerText.trim() === "Terminal");
      if (!b) return false;
      b.click();
      return true;
    });
    if (!opened) {
      console.log("  Terminal: button not found");
      bad++;
    } else {
      await page.waitForFunction(() => !!document.querySelector('[aria-label="Terminal"]'), { timeout: 5000 }).catch(() => {});
      await new Promise((r) => setTimeout(r, 700));
      const d = await page.evaluate(() => {
        const el = document.querySelector('[aria-label="Terminal"]');
        const m = document.querySelector("main");
        if (!el) return null;
        return {
          need: el.scrollWidth,
          have: el.clientWidth,
          mainNeed: m.scrollWidth,
          mainHave: m.clientWidth,
          clearance: parseInt(getComputedStyle(m).paddingBottom, 10) || 0,
          errored: el.innerText.includes("Tauri runtime not available") || el.innerText.includes("transformCallback"),
        };
      });
      await cspCheck(page, "Terminal");
      if (!d) {
        console.log("  Terminal: drawer never appeared");
        bad++;
      } else {
        const over = d.need > d.have + 1 || d.mainNeed > d.mainHave + 1;
        if (over || d.errored || d.clearance < 100) bad++;
        console.log(
          `  ${over || d.errored ? "\x1b[31mOVERFLOW\x1b[0m" : "\x1b[32mok      \x1b[0m"} ${"Terminal".padEnd(14)} needs ${String(d.need).padStart(4)} has ${d.have}` +
            (d.errored ? "  (the drawer rendered an error, not a shell)" : "") +
            (d.clearance < 100 ? `  (the page is not scrollable clear of it: ${d.clearance}px)` : "")
        );
      }
    }
    // The SSH Key Wizard's Test step with a failure that ends in a long key
    // path: the message must wrap inside the page, not run past it.
    {
      await page.evaluate(() => [...document.querySelectorAll("nav a")].find((x) => x.innerText.trim().startsWith("SSH Keys"))?.click());
      await new Promise((r) => setTimeout(r, 400));
      await page.type('input[placeholder="e.g., work, personal"]', "gitswitch_acme_platform_infrastructure_and_release_engineering").catch(() => {});
      await page.type('input[placeholder="your@email.com"]', "me@personal.test").catch(() => {});
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim().startsWith("Generate Key"))?.click());
      await page.waitForFunction(() => document.body.innerText.includes("Next: Test Connection"), { timeout: 4000 }).catch(() => {});
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("Next: Test Connection"))?.click());
      await page.waitForFunction(() => document.body.innerText.includes("Test SSH Connection"), { timeout: 4000 }).catch(() => {});
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.includes("Test SSH Connection"))?.click());
      const reached = await page.waitForFunction(() => document.body.innerText.includes("Permission denied for key"), { timeout: 4000 }).then(() => true).catch(() => false);
      const w = await page.evaluate(() => {
        const m = document.querySelector("main");
        const right = m.getBoundingClientRect().right;
        const past = [...m.querySelectorAll("*")].filter((el) => el.getBoundingClientRect().width > 0 && el.getBoundingClientRect().right > right + 1).length;
        const boxes = [...m.querySelectorAll("pre, p")].filter((el) => el.innerText.includes("id_ed25519_gitswitch_acme") && el.scrollWidth > el.clientWidth + 1 && getComputedStyle(el).overflowX !== "auto");
        return { past, spill: boxes.length, need: m.scrollWidth, have: m.clientWidth };
      });
      const over = !reached || w.past > 0 || w.spill > 0 || w.need > w.have + 1;
      if (over) bad++;
      console.log(`  ${over ? "\x1b[31mOVERFLOW\x1b[0m" : "\x1b[32mok      \x1b[0m"} ${"SSH Key Test".padEnd(14)} ${reached ? `a failure ending in a long key path stays inside the page (past the edge: ${w.past}, spilling boxes: ${w.spill})` : "the Test step's failure was not reached"}`);
    }
    // Auto Assign's reasons per repository (who can push, a failed check) must
    // stay on screen at half width: below 1024 px they used to be display:none.
    {
      await page.evaluate(() => {
        localStorage.setItem("gitswitch:autoassign.root", JSON.stringify("/repos"));
        window.__AUTO__ = true;
      });
      await page.evaluate(() => [...document.querySelectorAll("nav a")].find((x) => x.innerText.trim().startsWith("Profiles"))?.click());
      await new Promise((r) => setTimeout(r, 300));
      await page.evaluate(() => [...document.querySelectorAll("nav a")].find((x) => x.innerText.trim().startsWith("Auto Assign"))?.click());
      await new Promise((r) => setTimeout(r, 300));
      await page.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.innerText.trim() === "Scan")?.click());
      const shown = await page.waitForFunction(() => [...document.querySelectorAll("main span, main div")].some((el) => el.innerText.trim().startsWith("push ·") && el.getBoundingClientRect().width > 0), { timeout: 5000 }).then(() => true).catch(() => false);
      const w = await page.evaluate(() => ({ need: document.querySelector("main").scrollWidth, have: document.querySelector("main").clientWidth }));
      const over = !shown || w.need > w.have + 1;
      if (over) bad++;
      console.log(`  ${over ? "\x1b[31mOVERFLOW\x1b[0m" : "\x1b[32mok      \x1b[0m"} ${"Auto Assign".padEnd(14)} ${shown ? "the push · badge is on screen" : "no visible push · badge"}, needs ${w.need} has ${w.have}`);
      await page.evaluate(() => { window.__AUTO__ = false; });
    }
    // The profile form's folder and remote-rule inputs fill their rows: the Add
    // buttons sit at the right edge, not in the middle.
    {
      await page.goto(`${server.url}/profile/new`, { waitUntil: "networkidle0" });
      await new Promise((r) => setTimeout(r, 300));
      const f = await page.evaluate(() => {
        const gap = (input) => {
          const row = input?.closest("div.flex");
          const add = row && [...row.querySelectorAll("button")].pop();
          return row && add ? Math.round(row.getBoundingClientRect().right - add.getBoundingClientRect().right) : null;
        };
        const m = document.querySelector("main");
        return {
          folder: gap([...document.querySelectorAll("input")].find((i) => (i.placeholder || "").startsWith("…or type a path"))),
          remote: gap(document.querySelector('input[aria-label="Remote rule"]')),
          need: m.scrollWidth,
          have: m.clientWidth,
        };
      });
      const ok = f.folder !== null && f.folder <= 1 && f.remote !== null && f.remote <= 1 && f.need <= f.have + 1;
      if (!ok) bad++;
      console.log(`  ${ok ? "\x1b[32mok      \x1b[0m" : "\x1b[31mOVERFLOW\x1b[0m"} ${"Profile form".padEnd(14)} the inputs fill their rows (Add button short of the edge by: folder ${f.folder}px, remote ${f.remote}px), needs ${f.need} has ${f.have}`);
    }
    // What the rubber-band area past either end of a scroll shows is the
    // document's own background: it must be the app's dark, never white.
    const bg = await page.evaluate(() => [document.documentElement, document.body].map((el) => getComputedStyle(el).backgroundColor));
    const parse = (c) => {
      const m = c.match(/^rgba?\(([\d.]+), ([\d.]+), ([\d.]+)(?:, ([\d.]+))?\)$/);
      return m ? { r: +m[1], g: +m[2], b: +m[3], a: m[4] === undefined ? 1 : +m[4] } : null;
    };
    const dark = bg.every((c) => {
      const p = parse(c);
      return p !== null && p.a > 0 && p.r < 64 && p.g < 64 && p.b < 64;
    });
    if (!dark) bad++;
    await cspCheck(page, "the rest");
    console.log(`  ${dark ? "\x1b[32mok      \x1b[0m" : "\x1b[31mWHITE   \x1b[0m"} ${"Document".padEnd(14)} html/body background ${bg.join(" / ")}`);
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}
console.log(bad ? `\n\x1b[31m${bad} page/width combinations overflow\x1b[0m` : "\n\x1b[32mno page overflows at half-screen widths\x1b[0m");
process.exit(bad ? 1 : 0);
