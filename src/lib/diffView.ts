import { useSyncExternalStore } from "react";
import type { DiffLine } from "./api";

/**
 * How diffs are shown — one choice for the whole app, remembered: every diff
 * on screen follows it at once (the Changes page, History, a pull request's
 * files).
 */
export type DiffViewMode = "unified" | "split";

const KEY = "gitswitch:diff.view";
const listeners = new Set<() => void>();

function read(): DiffViewMode {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "null") === "split" ? "split" : "unified";
  } catch {
    return "unified";
  }
}

let current: DiffViewMode = read();

export function setDiffView(mode: DiffViewMode) {
  current = mode;
  try {
    localStorage.setItem(KEY, JSON.stringify(mode));
  } catch {
    // storage full or disabled — the choice still holds while the app runs
  }
  listeners.forEach((l) => l());
}

export function useDiffView(): [DiffViewMode, (m: DiffViewMode) => void] {
  const mode = useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current
  );
  return [mode, setDiffView];
}

/** One side of a side-by-side row: the line's number in its version, and its text. */
export interface SplitCell {
  no: number;
  text: string;
  tone: "context" | "del" | "add";
}

export type SplitRow =
  | { kind: "full"; tone: "hunk" | "meta"; text: string }
  | { kind: "pair"; left: SplitCell | null; right: SplitCell | null };

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/**
 * A unified diff as side-by-side rows: the old version on the left, the new
 * one on the right, each line with its number. Inside a hunk the lines are
 * read by the hunk's own counts (`@@ -a,b +c,d @@`), so a removed line that
 * happens to start with `--` is still a removed line. A run of removed lines
 * followed by added ones is paired row by row; a context line sits on both
 * sides. Headers and hunk lines span the table.
 *
 * Null when the diff is not one this can lay out faithfully (a combined
 * merge diff's `@@@`, or a line a hunk does not account for): the unified
 * view is shown instead.
 */
export function splitRows(lines: DiffLine[]): SplitRow[] | null {
  const out: SplitRow[] = [];
  let oldNo = 0;
  let newNo = 0;
  let oldLeft = 0;
  let newLeft = 0;
  let dels: SplitCell[] = [];
  let adds: SplitCell[] = [];
  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    for (let i = 0; i < n; i++) out.push({ kind: "pair", left: dels[i] ?? null, right: adds[i] ?? null });
    dels = [];
    adds = [];
  };
  for (const l of lines) {
    const t = l.text;
    if (t.startsWith("@@")) {
      flush();
      const m = HUNK.exec(t);
      if (!m) return null;
      oldNo = Number(m[1]);
      oldLeft = m[2] === undefined ? 1 : Number(m[2]);
      newNo = Number(m[3]);
      newLeft = m[4] === undefined ? 1 : Number(m[4]);
      out.push({ kind: "full", tone: "hunk", text: t });
      continue;
    }
    if (oldLeft > 0 || newLeft > 0) {
      const c = t.charAt(0);
      const body = t.slice(1);
      if (c === "-" && oldLeft > 0) {
        dels.push({ no: oldNo++, text: body, tone: "del" });
        oldLeft--;
        continue;
      }
      if (c === "+" && newLeft > 0) {
        adds.push({ no: newNo++, text: body, tone: "add" });
        newLeft--;
        continue;
      }
      if ((c === " " || t === "") && oldLeft > 0 && newLeft > 0) {
        flush();
        out.push({ kind: "pair", left: { no: oldNo++, text: body, tone: "context" }, right: { no: newNo++, text: body, tone: "context" } });
        oldLeft--;
        newLeft--;
        continue;
      }
      if (c === "\\") {
        flush();
        out.push({ kind: "full", tone: "meta", text: t });
        continue;
      }
      return null;
    }
    flush();
    out.push({ kind: "full", tone: "meta", text: t });
  }
  flush();
  return out;
}
