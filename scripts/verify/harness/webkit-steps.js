// Runs INSIDE the page, in the system's WebKit, as the body of an async
// function (WKWebView.callAsyncJavaScript). Opens the terminal drawer and
// measures what the browser harness measures: the rows drawn, the box they
// are drawn in, and the rows the shell was last told it has.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (f, ms) => {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    try { if (f()) return true; } catch { /* not there yet */ }
    await sleep(100);
  }
  return false;
};
const shape = () => {
  const drawer = document.querySelector('[aria-label="Terminal"]');
  const rowsEl = drawer && drawer.querySelector(".xterm-rows");
  const xterm = drawer && drawer.querySelector(".xterm");
  if (!rowsEl || !xterm) return null;
  const host = xterm.parentElement.getBoundingClientRect();
  const rows = rowsEl.children.length;
  const cell = rows ? rowsEl.children[0].getBoundingClientRect().height : 0;
  const lastBottom = rows ? rowsEl.lastElementChild.getBoundingClientRect().bottom : 0;
  const sized = window.__CALLS__.filter((c) => c.cmd === "terminal_open" || c.cmd === "terminal_resize");
  const last = sized[sized.length - 1];
  return {
    rows, cell: Math.round(cell * 100) / 100, boxHeight: Math.round(host.height),
    lastBottom: Math.round(lastBottom), boxBottom: Math.round(host.bottom), windowHeight: window.innerHeight,
    toldRows: last && last.args ? last.args.rows : null, toldBy: last ? last.cmd : null,
  };
};
const button = (pred) => [...document.querySelectorAll("button")].find(pred);
const out = { engine: navigator.userAgent, loaded: false, opened: false };
out.loaded = await until(() => document.body.innerText.includes("Changes"), 15000);
if (!out.loaded) return JSON.stringify(out);
button((b) => b.innerText.trim() === "Terminal").click();
out.opened = await until(() => !!document.querySelector('[aria-label="Terminal"] .xterm-rows'), 15000);
if (!out.opened) return JSON.stringify(out);
// The box changes shape while the shell may still be starting.
await sleep(300);
out.early = shape();
button((b) => b.innerText.includes("What it guards")).click();
await sleep(2800);
out.noteOpen = shape();
button((b) => b.innerText.includes("What it guards")).click();
await sleep(1200);
out.noteClosed = shape();
return JSON.stringify(out);
