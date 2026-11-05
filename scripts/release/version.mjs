#!/usr/bin/env node
// GitSwitch's version, in every file that carries it.
//
// The in-app updater offers a release when its version is newer than the
// running app's, and the app's version is what tauri.conf.json said at build
// time. A tag that disagrees with it ships an update that never shows up
// (tag ahead of the files) or one that is offered again after installing
// (files behind the tag) — so CI refuses such a tag, and this sets them all.
//
//   node scripts/release/version.mjs              print the version
//   node scripts/release/version.mjs 0.2.0        set it everywhere
//   node scripts/release/version.mjs --check v0.2.0   fail unless all say 0.2.0

import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

// Each: a file, and the one spot in it that holds the app's version. Edited
// as text so formatting and line endings stay exactly as they were.
const SPOTS = [
  ["src-tauri/tauri.conf.json", /^(  "version": ")([^"]+)(")/m],
  ["package.json", /^(  "version": ")([^"]+)(")/m],
  ["package-lock.json", /^(  "version": ")([^"]+)(")/m],
  ["package-lock.json", /^(    "": \{\r?\n      "name": "[^"]+",\r?\n      "version": ")([^"]+)(")/m],
  ["src-tauri/Cargo.toml", /^(\[package\]\r?\nname = "gitswitch"\r?\nversion = ")([^"]+)(")/m],
  ["src-tauri/Cargo.lock", /^(name = "gitswitch"\r?\nversion = ")([^"]+)(")/m],
];

function read() {
  return SPOTS.map(([file, re]) => {
    const m = re.exec(readFileSync(join(root, file), "utf8"));
    if (!m) throw new Error(`${file}: the version is not where it is expected`);
    return { file, version: m[2] };
  });
}

const [arg, value] = process.argv.slice(2);
try {
  if (!arg) {
    const found = new Set(read().map((s) => s.version));
    console.log([...found].join(" / "));
  } else if (arg === "--check") {
    const want = (value ?? "").replace(/^v/, "");
    if (!want) throw new Error("--check needs the tag, e.g. --check v0.2.0");
    const off = read().filter((s) => s.version !== want);
    if (off.length) {
      throw new Error(
        `the tag says ${want}, but ${off.map((s) => `${s.file} says ${s.version}`).join(", ")}. ` +
          `Run \`node scripts/release/version.mjs ${want}\`, commit, and tag that commit.`
      );
    }
    console.log(`every file says ${want}`);
  } else {
    if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(arg)) throw new Error(`${arg} is not a version like 0.2.0`);
    read();
    for (const [file, re] of SPOTS) {
      const path = join(root, file);
      writeFileSync(path, readFileSync(path, "utf8").replace(re, `$1${arg}$3`));
    }
    console.log(`set ${arg} in ${[...new Set(SPOTS.map(([f]) => f))].join(", ")}`);
  }
} catch (e) {
  console.error(`version: ${e.message}`);
  process.exit(1);
}
