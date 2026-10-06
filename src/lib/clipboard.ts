/**
 * Copy text, with a fallback for webviews that hide `navigator.clipboard`.
 *
 * Tauri's webview does not always expose the async clipboard API, so a bare
 * `navigator.clipboard.writeText` silently does nothing there. This lived
 * inside the commit detail panel and three other call sites went without it;
 * it belongs here so nobody has to remember.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to the textarea trick
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    ta.style.pointerEvents = "none";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}
