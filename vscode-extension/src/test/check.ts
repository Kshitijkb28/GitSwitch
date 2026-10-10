/** The smallest test runner: named checks, PASS/FAIL lines, a non-zero exit on any failure. */
let failed = 0;
const pending: Array<Promise<void>> = [];

export function check(label: string, fn: () => void | Promise<void>): void {
  const run = (async () => {
    try {
      await fn();
      console.log(`  PASS ${label}`);
    } catch (e) {
      failed++;
      console.log(`  FAIL ${label}\n     ${e instanceof Error ? e.message : e}`);
    }
  })();
  pending.push(run);
}

/** Run after the checks of a file are declared: one after another would hide nothing, so they run together. */
export async function done(): Promise<void> {
  await Promise.all(pending);
  console.log(`RESULT ${failed === 0 ? "all passed" : `${failed} failed`}`);
  if (failed) process.exitCode = 1;
}
