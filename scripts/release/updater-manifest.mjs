#!/usr/bin/env node
// latest.json for the in-app updater, built from a release's own files.
//
// The installed app asks
//   https://github.com/<repo>/releases/latest/download/latest.json
// and looks itself up under "<os>-<arch>-<installer>" first, then
// "<os>-<arch>" (tauri-plugin-updater 2.x). The installer is the bundle type
// written into the executable at build time, so a copy installed from the
// NSIS setup updates from the NSIS setup, a .deb install from the .deb, and
// so on: one entry per installer GitSwitch ships, plus a plain "<os>-<arch>"
// for a build that predates bundle types. Each entry is the installer's URL
// and the text of the .sig the bundler wrote next to it.
//
//   node updater-manifest.mjs --repo owner/name --tag v0.2.0 --dir sigs/ \
//        --assets assets.txt [--notes text] [--require windows-x86_64-nsis,...]
//
// --dir holds the release's *.sig files, --assets lists every file on the
// release (one name per line). Prints latest.json; fails, naming the file,
// when an installer has no signature, two different files claim one
// platform, or a --require'd platform has no installer.

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/** Which platform entries an installer file serves, by its name. */
export function platformsOf(name) {
  const arch = /aarch64|arm64/i.test(name) ? ["aarch64"] : /universal/i.test(name) ? ["aarch64", "x86_64"] : /x64|x86_64|amd64/i.test(name) ? ["x86_64"] : null;
  const kinds = [
    [/-setup\.exe$/i, "windows", "nsis", true],
    [/\.msi$/i, "windows", "msi", false],
    [/\.app\.tar\.gz$/i, "darwin", "app", true],
    [/\.AppImage$/, "linux", "appimage", true],
    [/\.deb$/i, "linux", "deb", false],
    [/\.rpm$/i, "linux", "rpm", false],
  ];
  const kind = kinds.find(([re]) => re.test(name));
  if (!kind) return [];
  const [, os, installer, plain] = kind;
  // A macOS build that names no architecture is the universal one.
  const archs = arch ?? (os === "darwin" ? ["aarch64", "x86_64"] : null);
  if (!archs) throw new Error(`${name}: no architecture in the file name`);
  // The plain "<os>-<arch>" goes to the installer most copies came from:
  // the NSIS setup (BUILD.md's Windows install), the AppImage (the only Linux
  // bundle that is one file anywhere), the .app.
  return archs.flatMap((a) => [`${os}-${a}-${installer}`, ...(plain ? [`${os}-${a}`] : [])]);
}

export function manifest({ repo, tag, version, notes, pubDate, assets, signatures, require = [] }) {
  const platforms = {};
  const from = {};
  for (const name of assets) {
    const keys = platformsOf(name);
    if (keys.length === 0) continue;
    const signature = signatures[`${name}.sig`];
    if (!signature) throw new Error(`${name} has no ${name}.sig on the release`);
    for (const key of keys) {
      // The same file uploaded twice (tauri-action's copy and the workflow's
      // renamed one) carries the same .sig: either URL serves it.
      if (from[key] && platforms[key].signature === signature.trim()) continue;
      if (from[key]) throw new Error(`${key} is claimed by both ${from[key]} and ${name}`);
      from[key] = name;
      platforms[key] = {
        signature: signature.trim(),
        url: `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`,
      };
    }
  }
  const missing = require.filter((k) => !platforms[k]);
  if (missing.length) throw new Error(`no installer on the release for ${missing.join(", ")}`);
  return { version, notes, pub_date: pubDate, platforms };
}

function main(argv) {
  const opt = {};
  for (let i = 0; i < argv.length; i += 2) opt[argv[i].replace(/^--/, "")] = argv[i + 1];
  for (const k of ["repo", "tag", "dir", "assets"]) if (!opt[k]) throw new Error(`--${k} is required`);
  const signatures = Object.fromEntries(
    readdirSync(opt.dir).filter((f) => f.endsWith(".sig")).map((f) => [f, readFileSync(join(opt.dir, f), "utf8")])
  );
  const assets = readFileSync(opt.assets, "utf8").split(/\r?\n/).map((s) => s.trim()).filter(Boolean);
  const out = manifest({
    repo: opt.repo,
    tag: opt.tag,
    version: opt.tag.replace(/^v/, ""),
    notes: opt.notes ?? `GitSwitch ${opt.tag}`,
    pubDate: new Date().toISOString(),
    assets,
    signatures,
    require: (opt.require ?? "").split(",").filter(Boolean),
  });
  process.stdout.write(JSON.stringify(out, null, 2) + "\n");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`updater-manifest: ${e.message}`);
    process.exit(1);
  }
}
