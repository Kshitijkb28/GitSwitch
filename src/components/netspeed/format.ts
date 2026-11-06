// Speeds and amounts in words, the same way the backend writes them for the
// taskbar and menu bar (decimal units, as networks count).

/** "12.4 MB/s" or "99.2 Mbps". Below one kilo it is still shown in kilos. */
export function speed(bytesPerS: number, bits: boolean): string {
  const units = bits ? ["bps", "Kbps", "Mbps", "Gbps"] : ["B/s", "KB/s", "MB/s", "GB/s"];
  let v = bits ? bytesPerS * 8 : bytesPerS;
  if (!Number.isFinite(v) || v < 0) v = 0;
  v /= 1000;
  let i = 1;
  while (v >= 999.95 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${v < 100 ? v.toFixed(1) : v.toFixed(0)} ${units[i]}`;
}

/** Bytes as an amount: "1.24 GB", "340 MB". */
export function amount(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  if (i === 0) return `${bytes} B`;
  return `${v < 10 ? v.toFixed(2) : v < 100 ? v.toFixed(1) : v.toFixed(0)} ${units[i]}`;
}

/** A link's speed in bits per second: "390 Mbps", "1 Gbps". */
export function link(bps: number | null | undefined): string | null {
  if (!bps || bps <= 0) return null;
  if (bps >= 1e9) return `${+(bps / 1e9).toFixed(1)} Gbps`;
  if (bps >= 1e6) return `${Math.round(bps / 1e6)} Mbps`;
  return `${Math.round(bps / 1e3)} Kbps`;
}

/** A speed-test result: bytes per second as Mbps. */
export function mbps(bytesPerS: number | null | undefined): string {
  if (bytesPerS == null) return "—";
  const v = (bytesPerS * 8) / 1e6;
  return `${v < 100 ? v.toFixed(1) : v.toFixed(0)} Mbps`;
}

/** A tidy top for an axis: 1, 2 or 5 times a power of ten, at or above `v`. */
export function niceMax(v: number): number {
  if (!(v > 0)) return 1000;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 2, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}
