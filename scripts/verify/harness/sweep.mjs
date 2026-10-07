// Every page at half-screen widths, measuring the element that actually
// scrolls. `<main>` has overflow-y-auto, so it scrolls horizontally on its own
// and documentElement never reports overflow — measuring the wrong element once
// hid a page that was visibly clipped.
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
const PAGES = ["Profiles", "SSH Keys", "GitHub Auth", "Repositories", "Clone", "Repository", "Auto Assign", "Doctor", "Commit Audit", "Settings"];

const server = await serve();
const browser = await launch();
let bad = 0;
try {
  for (const width of [700, 900]) {
    console.log(`\n\x1b[1m${width}px (half screen)\x1b[0m`);
    const page = await browser.newPage();
    await page.setViewport({ width, height: 900 });
    await page.evaluateOnNewDocument((repos, st, d1, subs, stashes, submods) => {
      localStorage.clear();
      // The Clone page takes its readings only once it has a URL and a folder.
      localStorage.setItem("gitswitch:sparse.url", JSON.stringify("git@github.com:me/big-data.git"));
      localStorage.setItem("gitswitch:sparse.parentDir", JSON.stringify("/repos"));
      localStorage.setItem("gitswitch:changes.repo", JSON.stringify("/repos/gitswitch"));
      localStorage.setItem("gitswitch:history.repo", JSON.stringify("/repos/gitswitch"));
      const profiles = [
        { id: "p1", name: "Kirmada", git_name: "Me", git_email: "me@personal.test", ssh_key_path: "/Users/me/.ssh/id_ed25519_personal", is_default: true, allow_push: true, signing_enabled: false, directories: ["/Users/me/Documents/Acme/gitswitch", "/Users/me/Documents/very/long/folder/path/that/goes/on"], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
        { id: "p2", name: "Work", git_name: "Me At Work", git_email: "me@work.test", ssh_key_path: null, is_default: false, allow_push: false, signing_enabled: true, directories: ["/Users/me/Documents/Work"], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
      ];
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
          switch (cmd) {
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
            default: return Promise.resolve([]);
          }
        },
      };
    }, REPOS, status, DIFF_TEXT, SUB_STATUSES, STASHES, SUBMODULES);
    // Start off a page that is not swept, so the first stop (Profiles, at "/") mounts fresh on its click.
    await page.goto(`${server.url}/profile/new`, { waitUntil: "networkidle0" });

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
    console.log(`  ${dark ? "\x1b[32mok      \x1b[0m" : "\x1b[31mWHITE   \x1b[0m"} ${"Document".padEnd(14)} html/body background ${bg.join(" / ")}`);
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}
console.log(bad ? `\n\x1b[31m${bad} page/width combinations overflow\x1b[0m` : "\n\x1b[32mno page overflows at half-screen widths\x1b[0m");
process.exit(bad ? 1 : 0);
