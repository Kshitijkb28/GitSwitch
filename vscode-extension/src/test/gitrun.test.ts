import * as assert from "node:assert/strict";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { check, done } from "./check";
import { fetchWatched, git } from "../gitrun";

// Fake gits that behave like a fetch would, whatever they are asked. Scripts
// run by this same runtime (node or bun), not shell scripts: Windows cannot
// start a `#!/bin/sh` file as a program.
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "gitswitch-gitrun-"));
const fake = (name: string, body: string) => {
  const f = path.join(dir, `${name}.js`);
  fs.writeFileSync(f, `if (["maintenance", "gc"].includes(process.argv[2])) process.exit(0);\n${body}\n`);
  return { program: process.execPath, programArgs: [f] };
};
const slowButMoving = fake(
  "moving",
  'let i = 0; const tick = () => { process.stderr.write("Receiving objects: " + i * 20 + "%\\n"); if (++i < 5) setTimeout(tick, 1000); else setTimeout(() => process.exit(0), 1000); }; tick();'
);
const silent = fake("silent", "setTimeout(() => {}, 30_000);");
const prompt = fake("prompt", 'process.stdout.write(process.env.GIT_TERMINAL_PROMPT ?? "");');

check("a fetch that keeps reporting progress is not stopped for taking long", async () => {
  // 5 s of progress, a 1.5 s silence window: a fixed 10 s cap would be the old way.
  await fetchWatched(dir, { ...slowButMoving, stallMs: 1500 });
});
check("a fetch that goes silent is stopped after the silence window", async () => {
  const started = Date.now();
  await assert.rejects(fetchWatched(dir, { ...silent, stallMs: 1000 }), /no progress for 1 s/);
  assert.ok(Date.now() - started < 8000, `took ${Date.now() - started} ms`);
});
check("a fetch can be cancelled", async () => {
  const abort = new AbortController();
  setTimeout(() => abort.abort(), 300);
  await assert.rejects(fetchWatched(dir, { ...silent, stallMs: 60_000, signal: abort.signal }), /cancelled/);
});
check("git cannot wait on a prompt", async () => {
  assert.equal(await git(dir, [], { ...prompt }), "0");
});

void done();
