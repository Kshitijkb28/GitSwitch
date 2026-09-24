// Every page at half-screen widths, measuring the element that actually
// scrolls. `<main>` has overflow-y-auto, so it scrolls horizontally on its own
// and documentElement never reports overflow — measuring the wrong element once
// hid a page that was visibly clipped.
import { launch, serve, DIST_OK } from "./browser.mjs";
import { REPOS, SCENARIOS, DIFF_TEXT, SUB_STATUSES, STASHES } from "./fixtures.mjs";

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
const PAGES = ["Profiles", "SSH Keys", "GitHub Auth", "Repositories", "Clone", "Changes", "Auto Assign", "Doctor", "History", "Commit Audit", "Settings"];

const server = await serve();
const browser = await launch();
let bad = 0;
try {
  for (const width of [700, 900]) {
    console.log(`\n\x1b[1m${width}px (half screen)\x1b[0m`);
    const page = await browser.newPage();
    await page.setViewport({ width, height: 900 });
    await page.evaluateOnNewDocument((repos, st, d1, subs, stashes) => {
      localStorage.clear();
      localStorage.setItem("gitswitch:changes.repo", JSON.stringify("/repos/gitswitch"));
      localStorage.setItem("gitswitch:history.repo", JSON.stringify("/repos/gitswitch"));
      const profiles = [
        { id: "p1", name: "Kirmada", git_name: "Me", git_email: "me@personal.test", ssh_key_path: "/Users/me/.ssh/id_ed25519_personal", is_default: true, allow_push: true, signing_enabled: false, directories: ["/Users/me/Documents/Ethara/gitswitch", "/Users/me/Documents/very/long/folder/path/that/goes/on"], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
        { id: "p2", name: "Work", git_name: "Me At Work", git_email: "me@work.test", ssh_key_path: null, is_default: false, allow_push: false, signing_enabled: true, directories: ["/Users/me/Documents/Harbor"], created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" },
      ];
      window.__TAURI_INTERNALS__ = {
        invoke: (cmd) => {
          switch (cmd) {
            case "get_profiles": return Promise.resolve(profiles);
            case "history_list_repos": return Promise.resolve(repos);
            case "changes_repo_status": return Promise.resolve(st);
            case "changes_submodule_statuses": return Promise.resolve(subs);
            case "changes_stash_list": return Promise.resolve(stashes);
            case "changes_file_diff": return Promise.resolve(d1);
            case "get_current_git_config": return Promise.resolve("[user]\n\tname = Me\n\temail = me@personal.test");
            case "allowed_signers_path": return Promise.resolve("/Users/me/.ssh/allowed_signers");
            case "local_clone_index": return Promise.resolve({ by_path: {}, by_repo: {} });
            case "list_remote_repos": return Promise.resolve({ repos: [], sso_hidden_orgs: 0, truncated: false });
            case "list_remote_repos_page": return Promise.resolve({ listing: { account: "a", login: "me", suggested_profile_id: null, repos: [], sso_hidden_orgs: 0, truncated: false }, page: 1, per_page: 10, has_more: false });
            case "history_branches": return Promise.resolve([{ name: "main", is_current: true, is_remote: false, upstream: "origin/main", ahead: 0, behind: 0, tip: "aaa", short_tip: "aaa", last_author: "Me", last_date: "2026-01-01T00:00:00Z", subject: "x" }]);
            case "history_page": return Promise.resolve({ commits: [], total: 0, has_more: false });
            default: return Promise.resolve([]);
          }
        },
      };
    }, REPOS, status, DIFF_TEXT, SUB_STATUSES, STASHES);
    await page.goto(`${server.url}/`, { waitUntil: "networkidle0" });

    for (const name of PAGES) {
      const clicked = await page.evaluate((n) => {
        const a = [...document.querySelectorAll("a")].find((x) => x.innerText.trim() === n);
        if (a) { a.click(); return true; }
        return false;
      }, name);
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
      console.log(`  ${over ? "\x1b[31mOVERFLOW\x1b[0m" : "\x1b[32mok      \x1b[0m"} ${name.padEnd(14)} needs ${String(r.need).padStart(4)} has ${r.have}`);
      for (const c of r.culprits) console.log(`             ${c}`);
    }
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}
console.log(bad ? `\n\x1b[31m${bad} page/width combinations overflow\x1b[0m` : "\n\x1b[32mno page overflows at half-screen widths\x1b[0m");
process.exit(bad ? 1 : 0);
