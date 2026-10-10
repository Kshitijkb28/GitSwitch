import * as assert from "node:assert/strict";
import { check, done } from "./check";
import { emailsMatch, folderProfile, originPairs, profileFromOrigins, profilesPath, resolveIdentity, Profile } from "../resolve";

const p = (id: string, name: string, email: string, directories: string[] = [], is_default = false): Profile => ({
  id, name, git_name: name, git_email: email, ssh_key_path: null, is_default, allow_push: true, directories,
});
const personal = p("p-default", "Personal", "me@personal.test", [], true);
const work = p("p-work", "Work", "me@work.test", ["/dev/work"]);
const inner = p("p-inner", "Inner", "inner@work.test", ["/dev/work/inner/"]);
const acme = p("p-acme", "Acme", "acme@example.test");
const all = [personal, work, inner, acme];

check("the longest folder wins", () => {
  assert.equal(folderProfile("/dev/work/inner/api", all)?.name, "Inner");
  assert.equal(folderProfile("/dev/work/api", all)?.name, "Work");
  assert.equal(folderProfile("/dev/workshop/api", all), null);
});
check("  whatever the separators and trailing slashes", () => {
  const win = p("w", "Win", "w@x.test", ["C:\\Users\\me\\Work\\"]);
  assert.equal(folderProfile("C:/Users/me/Work/repo", [win])?.name, "Win");
});
check("git's origins are read in pairs", () => {
  const out = "file:/home/me/.gitconfig\0me@personal.test\0file:/home/me/.local/share/com.gitswitch.app/gitconfigs/p-acme.gitconfig\0acme@example.test\0";
  assert.deepEqual(originPairs(out), [
    ["file:/home/me/.gitconfig", "me@personal.test"],
    ["file:/home/me/.local/share/com.gitswitch.app/gitconfigs/p-acme.gitconfig", "acme@example.test"],
  ]);
});
check("a remote-rule origin names its profile, the last include winning", () => {
  const pairs: Array<[string, string]> = [
    ["file:/home/me/.gitconfig", "me@personal.test"],
    ["file:/x/com.gitswitch.app/gitconfigs/p-work.gitconfig", "me@work.test"],
    ["file:/x/com.gitswitch.app/gitconfigs/p-acme.gitconfig", "acme@example.test"],
  ];
  assert.equal(profileFromOrigins(pairs, all)?.name, "Acme");
});
check("  on Windows paths too", () => {
  const pairs: Array<[string, string]> = [["file:C:/Users/x/AppData/Roaming/com.gitswitch.app/gitconfigs/p-acme.gitconfig", "a@x"]];
  assert.equal(profileFromOrigins(pairs, all)?.name, "Acme");
});
check("  looking past a repository's own .git/config read after the includes", () => {
  const pairs: Array<[string, string]> = [
    ["file:/x/com.gitswitch.app/gitconfigs/p-acme.gitconfig", "acme@example.test"],
    ["file:.git/config", "ACME@EXAMPLE.TEST"],
  ];
  assert.equal(profileFromOrigins(pairs, all)?.name, "Acme");
});
check("  and an include of an unknown profile names none", () => {
  assert.equal(profileFromOrigins([["file:/x/gitconfigs/gone.gitconfig", "a@x"]], all), null);
});
check("emails are compared without letter case or spaces around", () => {
  assert.ok(emailsMatch(" Me@Work.Test ", "me@work.test"));
  assert.ok(!emailsMatch("me@work.test", "me@work.test.org"));
});
check("the profiles file is where the app keeps it, per platform", () => {
  assert.equal(profilesPath("/Users/me", "darwin", {}), "/Users/me/Library/Application Support/com.gitswitch.app/profiles.json");
  assert.equal(profilesPath("/home/me", "linux", {}), "/home/me/.local/share/com.gitswitch.app/profiles.json");
  assert.equal(profilesPath("/home/me", "linux", { XDG_DATA_HOME: "/data" }), "/data/com.gitswitch.app/profiles.json");
  // dirs ignores a relative XDG_DATA_HOME, and so must this.
  assert.equal(profilesPath("/home/me", "linux", { XDG_DATA_HOME: "relative" }), "/home/me/.local/share/com.gitswitch.app/profiles.json");
});

const fakeGit = (email: string, origins: string) => async (_cwd: string, args: string[]) => {
  if (args.includes("--show-origin")) return origins;
  if (args.includes("user.email")) return email;
  if (args.includes("user.name")) return "Someone";
  return "";
};
check("a clone of a remote-rule owner outside every folder is that profile's, with no mismatch", async () => {
  const r = await resolveIdentity("/anywhere/thing", fakeGit("acme@example.test", "file:/h/.gitconfig\0me@personal.test\0file:/h/gitconfigs/p-acme.gitconfig\0acme@example.test\0"), all);
  assert.equal(r.profile?.name, "Acme");
  assert.equal(r.mismatch, false);
});
check("  a local override that differs only in letter case is no mismatch", async () => {
  const r = await resolveIdentity("/anywhere/thing", fakeGit("ACME@EXAMPLE.TEST", "file:/h/gitconfigs/p-acme.gitconfig\0acme@example.test\0file:.git/config\0ACME@EXAMPLE.TEST\0"), all);
  assert.equal(r.profile?.name, "Acme");
  assert.equal(r.mismatch, false);
});
check("  inside a folder profile's folder, the folder profile", async () => {
  const r = await resolveIdentity("/dev/work/api", fakeGit("me@work.test", "file:/h/gitconfigs/p-work.gitconfig\0me@work.test\0"), all);
  assert.equal(r.profile?.name, "Work");
});
check("  an unrelated clone: the default profile, and a real difference is a mismatch", async () => {
  const r = await resolveIdentity("/elsewhere/x", fakeGit("someone@else.test", "file:/h/.gitconfig\0someone@else.test\0"), all);
  assert.equal(r.profile?.name, "Personal");
  assert.equal(r.mismatch, true);
});

void done();
