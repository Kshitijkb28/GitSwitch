// Shared plumbing for the headless checks: serve the built app, launch Chrome.
import puppeteer from "puppeteer-core";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const ROOT = path.resolve(HERE, "..", "..", "..");
export const DIST = path.join(ROOT, "dist");
export const DIST_OK = fs.existsSync(path.join(DIST, "index.html"));

const CHROME =
  process.env.CHROME ??
  [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
  ].find((p) => fs.existsSync(p));

const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
};

/** Serve dist/ on a free port with an SPA fallback. */
export function serve() {
  const server = http.createServer((req, res) => {
    const url = req.url.split("?")[0];
    let file = path.join(DIST, url === "/" ? "index.html" : url);
    if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) {
      file = path.join(DIST, "index.html");
    }
    res.writeHead(200, { "Content-Type": TYPES[path.extname(file)] ?? "text/plain" });
    res.end(fs.readFileSync(file));
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      resolve({ url: `http://127.0.0.1:${port}`, close: () => server.close() });
    });
  });
}

export async function launch() {
  if (!CHROME) {
    throw new Error("No Chrome found — set CHROME=/path/to/chrome");
  }
  return puppeteer.launch({ executablePath: CHROME, headless: "new", args: ["--no-sandbox"] });
}

/** A tiny pass/fail tally shared by the suites. */
export function tally() {
  let pass = 0;
  let fail = 0;
  const failures = [];
  return {
    ok(label, cond, detail = "") {
      if (cond) {
        pass++;
        console.log(`  \x1b[32mPASS\x1b[0m ${label}`);
      } else {
        fail++;
        failures.push(label);
        console.log(`  \x1b[31mFAIL\x1b[0m ${label}${detail ? `\n       ${detail}` : ""}`);
      }
    },
    section(t) {
      console.log(`\n\x1b[1m== ${t}\x1b[0m`);
    },
    done() {
      console.log(`\n\x1b[1mRESULT\x1b[0m  ${pass} passed, ${fail} failed`);
      if (failures.length) console.log("failed: " + failures.join(" | "));
      return fail === 0;
    },
  };
}
