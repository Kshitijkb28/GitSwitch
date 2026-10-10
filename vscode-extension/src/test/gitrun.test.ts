import * as assert from "node:assert/strict";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { check, done } from "./check";
import { fetchWatched, git } from "../gitrun";

// Fake gits: shell scripts that behave like a fetch would, whatever they are asked.
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "gitswitch-gitrun-"));
const fake = (name: string, body: string) => {
  const f = path.join(dir, name);
  fs.writeFileSync(f, `#!/bin/sh\ncase "$1" in maintenance|gc) exit 0;; esac\n${body}\n`, { mode: 0o755 });
  return f;
};
const slowButMoving = fake("moving", 'i=0; while [ $i -lt 5 ]; do echo "Receiving objects: $((i*20))%" >&2; sleep 1; i=$((i+1)); done; exit 0');
const silent = fake("silent", "sleep 30");
const prompt = fake("prompt", 'printf "%s" "$GIT_TERMINAL_PROMPT"');

check("a fetch that keeps reporting progress is not stopped for taking long", async () => {
  // 5 s of progress, a 1.5 s silence window: a fixed 10 s cap would be the old way.
  await fetchWatched(dir, { program: slowButMoving, stallMs: 1500 });
});
check("a fetch that goes silent is stopped after the silence window", async () => {
  const started = Date.now();
  await assert.rejects(fetchWatched(dir, { program: silent, stallMs: 1000 }), /no progress for 1 s/);
  assert.ok(Date.now() - started < 8000, `took ${Date.now() - started} ms`);
});
check("a fetch can be cancelled", async () => {
  const abort = new AbortController();
  setTimeout(() => abort.abort(), 300);
  await assert.rejects(fetchWatched(dir, { program: silent, stallMs: 60_000, signal: abort.signal }), /cancelled/);
});
check("git cannot wait on a prompt", async () => {
  assert.equal(await git(dir, [], { program: prompt }), "0");
});

void done();
