// node scripts/release/updater-manifest.test.mjs
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { manifest, platformsOf } from "./updater-manifest.mjs";

let passed = 0;
const ok = (name, fn) => {
  fn();
  passed++;
  console.log(`ok - ${name}`);
};

// What a tagged build puts on the release (tauri bundler names; the macOS
// tarball is renamed by the workflow, which builds one universal app).
const ASSETS = [
  "GitSwitch_0.2.0_x64-setup.exe",
  "GitSwitch_0.2.0_x64_en-US.msi",
  "GitSwitch_0.2.0_universal.app.tar.gz",
  "GitSwitch_0.2.0_universal.dmg",
  "GitSwitch_0.2.0_amd64.AppImage",
  "GitSwitch_0.2.0_amd64.deb",
  "GitSwitch-0.2.0-1.x86_64.rpm",
  "gitswitch-0.2.0.vsix",
  "SHA256SUMS.txt",
];
const SIGNED = ASSETS.filter((a) => platformsOf(a).length > 0);
const signatures = Object.fromEntries(SIGNED.map((a) => [`${a}.sig`, `sig-of-${a}\n`]));
const ALL = [
  "windows-x86_64-nsis", "windows-x86_64", "windows-x86_64-msi",
  "darwin-aarch64-app", "darwin-aarch64", "darwin-x86_64-app", "darwin-x86_64",
  "linux-x86_64-appimage", "linux-x86_64", "linux-x86_64-deb", "linux-x86_64-rpm",
];
const build = (over = {}) =>
  manifest({ repo: "Kshitijkb28/GitSwitch", tag: "v0.2.0", version: "0.2.0", notes: "n", pubDate: "2026-11-01T00:00:00Z", assets: ASSETS, signatures, require: ALL, ...over });

ok("every installer GitSwitch ships gets its own entry, and each OS a plain one", () => {
  assert.deepEqual(Object.keys(build().platforms).sort(), [...ALL].sort());
});

ok("an install updates from the installer it came from", () => {
  const p = build().platforms;
  assert.equal(p["windows-x86_64-nsis"].signature, "sig-of-GitSwitch_0.2.0_x64-setup.exe");
  assert.equal(p["windows-x86_64-msi"].signature, "sig-of-GitSwitch_0.2.0_x64_en-US.msi");
  assert.equal(p["linux-x86_64-deb"].signature, "sig-of-GitSwitch_0.2.0_amd64.deb");
  assert.equal(p["linux-x86_64-rpm"].signature, "sig-of-GitSwitch-0.2.0-1.x86_64.rpm");
  assert.equal(p["linux-x86_64-appimage"].signature, "sig-of-GitSwitch_0.2.0_amd64.AppImage");
});

ok("the plain entries are the NSIS setup, the AppImage and the app", () => {
  const p = build().platforms;
  assert.equal(p["windows-x86_64"].url, p["windows-x86_64-nsis"].url);
  assert.equal(p["linux-x86_64"].url, p["linux-x86_64-appimage"].url);
  assert.equal(p["darwin-x86_64"].url, p["darwin-aarch64"].url);
});

ok("a universal macOS app serves both Apple Silicon and Intel", () => {
  const p = build().platforms;
  assert.equal(p["darwin-aarch64-app"].url, "https://github.com/Kshitijkb28/GitSwitch/releases/download/v0.2.0/GitSwitch_0.2.0_universal.app.tar.gz");
  assert.equal(p["darwin-x86_64-app"].url, p["darwin-aarch64-app"].url);
});

ok("URLs point at the tag (they keep working once the draft is published)", () => {
  for (const { url } of Object.values(build().platforms)) assert.match(url, /^https:\/\/github\.com\/Kshitijkb28\/GitSwitch\/releases\/download\/v0\.2\.0\/[^/]+$/);
});

ok("the dmg, the extension and the checksums are not updater files", () => {
  assert.deepEqual(platformsOf("GitSwitch_0.2.0_universal.dmg"), []);
  assert.deepEqual(platformsOf("gitswitch-0.2.0.vsix"), []);
  assert.deepEqual(platformsOf("SHA256SUMS.txt"), []);
});

ok("an installer without its signature is refused, by name", () => {
  const { ["GitSwitch_0.2.0_amd64.deb.sig"]: _, ...rest } = signatures;
  assert.throws(() => build({ signatures: rest }), /GitSwitch_0\.2\.0_amd64\.deb has no GitSwitch_0\.2\.0_amd64\.deb\.sig/);
});

ok("one file uploaded under two names is one entry", () => {
  const assets = [...ASSETS, "GitSwitch_universal.app.tar.gz"];
  const sigs = { ...signatures, "GitSwitch_universal.app.tar.gz.sig": signatures["GitSwitch_0.2.0_universal.app.tar.gz.sig"] };
  assert.match(build({ assets, signatures: sigs }).platforms["darwin-aarch64-app"].url, /GitSwitch_0\.2\.0_universal\.app\.tar\.gz$/);
});

ok("two different files claiming one platform are refused", () => {
  const assets = [...ASSETS, "GitSwitch_0.2.0_x64-old-setup.exe"];
  assert.throws(() => build({ assets, signatures: { ...signatures, "GitSwitch_0.2.0_x64-old-setup.exe.sig": "x" } }), /windows-x86_64-nsis is claimed by both/);
});

ok("a release missing a platform is refused instead of leaving those installs behind", () => {
  assert.throws(() => build({ assets: ASSETS.filter((a) => !a.endsWith(".rpm")) }), /no installer on the release for linux-x86_64-rpm/);
});

ok("the command line reads the release's file list and its .sig files", () => {
  const dir = mkdtempSync(join(tmpdir(), "updater-manifest-"));
  try {
    for (const [f, s] of Object.entries(signatures)) writeFileSync(join(dir, f), s);
    writeFileSync(join(dir, "assets.txt"), ASSETS.join("\r\n") + "\r\n");
    const script = join(dirname(fileURLToPath(import.meta.url)), "updater-manifest.mjs");
    const out = JSON.parse(execFileSync(process.execPath, [script, "--repo", "Kshitijkb28/GitSwitch", "--tag", "v0.2.0", "--dir", dir, "--assets", join(dir, "assets.txt"), "--require", ALL.join(",")], { encoding: "utf8" }));
    assert.equal(out.version, "0.2.0");
    assert.ok(!Number.isNaN(Date.parse(out.pub_date)));
    assert.deepEqual(Object.keys(out.platforms).sort(), [...ALL].sort());
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

console.log(`\n${passed} passed, 0 failed`);
