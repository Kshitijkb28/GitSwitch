import { execFile, spawn } from "child_process";

/**
 * Running git the way the desktop app does (src-tauri/src/git_exec.rs), free
 * of the `vscode` module so it can be tested on its own.
 *
 * - Never waits on a prompt: a password or passphrase question has nobody to
 *   answer it here, so git is told there is no terminal and no helper.
 * - A read has a ceiling; a fetch does not. A fetch of a big repository on a
 *   slow link can take minutes and is fine while it keeps reporting progress,
 *   so it is stopped only after a silence — as the app stops one.
 */

/** The environment git runs with: no prompt it could wait on forever. */
export function hardenedEnv(base: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...base, GIT_TERMINAL_PROMPT: "0", SSH_ASKPASS_REQUIRE: "never" };
  delete env.GIT_ASKPASS;
  delete env.SSH_ASKPASS;
  return env;
}

/** A read: the app's READ_TIMEOUT. */
export function git(
  cwd: string,
  args: string[],
  { timeoutMs = 30_000, program = "git", programArgs = [] as string[] } = {}
): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(program, [...programArgs, ...args], { cwd, timeout: timeoutMs, env: hardenedEnv(), maxBuffer: 16 * 1024 * 1024 }, (err, stdout, stderr) => {
      if (err) reject(new Error(String(stderr).trim() || err.message));
      else resolve(String(stdout).trim());
    });
  });
}

export interface FetchOptions {
  /** Stop after this long without a byte of output. */
  stallMs?: number;
  program?: string;
  /** Put before git's own arguments: a stand-in run by an interpreter (the tests' fake gits). */
  programArgs?: string[];
  signal?: AbortSignal;
}

/**
 * `git fetch --all`, watched: every line git prints restarts the silence
 * clock, and there is no total ceiling. `fetch.unpackLimit=1` because git
 * prints nothing at all while it unpacks a small fetch; automatic
 * housekeeping is left out of the fetch and run afterwards, best effort, so
 * a long `gc` is not mistaken for a stuck download.
 */
export function fetchWatched(cwd: string, { stallMs = 180_000, program = "git", programArgs = [], signal }: FetchOptions = {}): Promise<void> {
  return new Promise((resolve, reject) => {
    // Its own process group (unix): git runs ssh or git-remote-https as
    // children that hold its pipes, so stopping git alone would leave them
    // fetching — and the wait for the pipes to close as long as theirs.
    const windows = process.platform === "win32";
    const child = spawn(
      program,
      [...programArgs, "-c", "fetch.unpackLimit=1", "-c", "maintenance.auto=false", "-c", "gc.auto=0", "fetch", "--all", "--progress"],
      { cwd, env: hardenedEnv(), detached: !windows }
    );
    let lastErr = "";
    let stopped: string | null = null;
    let timer: NodeJS.Timeout | undefined;
    const killTree = (sig: NodeJS.Signals) => {
      if (child.pid === undefined) return;
      if (windows) execFile("taskkill", ["/pid", String(child.pid), "/T", "/F"], () => {});
      else {
        try {
          process.kill(-child.pid, sig);
        } catch {
          /* already gone */
        }
      }
    };
    const stop = (why: string) => {
      if (stopped) return;
      stopped = why;
      killTree("SIGTERM");
      setTimeout(() => killTree("SIGKILL"), 5_000).unref();
    };
    const arm = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => stop(`no progress for ${Math.round(stallMs / 1000)} s`), stallMs);
    };
    const onAbort = () => stop("cancelled");
    signal?.addEventListener("abort", onAbort);
    arm();
    child.stdout.on("data", arm);
    child.stderr.on("data", (chunk: Buffer) => {
      arm();
      const lines = chunk.toString().split(/[\r\n]+/).map((l) => l.trim()).filter(Boolean);
      if (lines.length) lastErr = lines[lines.length - 1];
    });
    child.on("error", (e) => {
      if (timer) clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      reject(e);
    });
    child.on("close", (code) => {
      if (timer) clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      if (stopped) {
        reject(new Error(stopped));
      } else if (code === 0) {
        housekeeping(cwd, program, programArgs);
        resolve();
      } else {
        reject(new Error(lastErr || `git fetch exited with ${code}`));
      }
    });
  });
}

/** What the fetch left out: `git maintenance run --auto`, else `gc --auto`. Nobody waits on it. */
function housekeeping(cwd: string, program: string, programArgs: string[]) {
  execFile(program, [...programArgs, "maintenance", "run", "--auto", "--quiet"], { cwd, env: hardenedEnv() }, (err) => {
    if (err) execFile(program, [...programArgs, "gc", "--auto", "--quiet"], { cwd, env: hardenedEnv() }, () => {});
  });
}
