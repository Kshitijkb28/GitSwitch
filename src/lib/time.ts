/**
 * A git timestamp in the person's local time, to the minute — "2026-10-05
 * 18:10" — with a title that carries the full stamp and how long ago it was.
 * Tolerant of git's `%aI` / `%cI` and for-each-ref's shapes; a value that is
 * not a date is shown as given rather than as "Invalid Date".
 */
export function formatWhen(iso: string, now: Date = new Date()): { text: string; title: string } {
  const d = new Date(iso);
  if (!iso || Number.isNaN(d.getTime())) return { text: iso, title: iso };
  const pad = (n: number) => String(n).padStart(2, "0");
  const text = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const secs = Math.round((now.getTime() - d.getTime()) / 1000);
  const ago =
    secs < 0
      ? "in the future"
      : secs < 60
        ? "just now"
        : secs < 3600
          ? `${Math.floor(secs / 60)} min ago`
          : secs < 86400
            ? `${Math.floor(secs / 3600)} h ago`
            : `${Math.floor(secs / 86400)} d ago`;
  return { text, title: `${iso} · ${ago}` };
}
