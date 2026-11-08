import { useEffect, useMemo, useRef, useState } from "react";
import { speed, niceMax } from "./format";

// Download and upload are categorical slots 1 and 2 (blue, orange), the dark
// steps — validated on the card surface (#1f1f22): CVD ΔE 26.8, contrast ≥ 3:1.
export const DOWN_COLOUR = "#3987e5";
export const UP_COLOUR = "#d95926";

const HEIGHT = 168;
const PAD = { top: 10, right: 12, bottom: 22, left: 64 };

function useWidth<T extends HTMLElement>(): [React.RefObject<T>, number] {
  const ref = useRef<T>(null);
  const [w, setW] = useState(600);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(el.clientWidth || 600));
    ro.observe(el);
    setW(el.clientWidth || 600);
    return () => ro.disconnect();
  }, []);
  return [ref as React.RefObject<T>, w];
}

/**
 * The last couple of minutes: download and upload as two lines over light
 * washes, one axis (both are speeds). Hovering (or the arrow keys) shows both
 * values at that moment; the numbers are also in a table.
 */
export function SpeedGraph({ history, bits }: { history: [number, number, number][]; bits: boolean }) {
  const [boxRef, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const plotW = Math.max(100, width - PAD.left - PAD.right);
  const plotH = HEIGHT - PAD.top - PAD.bottom;

  const { top, xs, yDown, yUp, t0, t1 } = useMemo(() => {
    const top = niceMax(Math.max(1, ...history.map((h) => Math.max(h[1], h[2]))) * 1.1);
    const t0 = history.length ? history[0][0] : 0;
    const t1 = history.length ? history[history.length - 1][0] : 1;
    const span = Math.max(1, t1 - t0);
    const xs = history.map((h) => PAD.left + ((h[0] - t0) / span) * plotW);
    const y = (v: number) => PAD.top + plotH - (Math.min(v, top) / top) * plotH;
    return { top, xs, yDown: history.map((h) => y(h[1])), yUp: history.map((h) => y(h[2])), t0, t1 };
  }, [history, plotW, plotH]);

  const base = PAD.top + plotH;
  const line = (ys: number[]) => ys.map((y, i) => `${i ? "L" : "M"}${xs[i].toFixed(1)},${y.toFixed(1)}`).join("");
  const area = (ys: number[]) => (ys.length ? `${line(ys)}L${xs[xs.length - 1].toFixed(1)},${base}L${xs[0].toFixed(1)},${base}Z` : "");
  const ago = (t: number) => {
    const s = Math.round((t1 - t) / 1000);
    return s <= 0 ? "now" : `${s} s ago`;
  };

  function pick(clientX: number, svg: SVGSVGElement) {
    if (!xs.length) return;
    const x = clientX - svg.getBoundingClientRect().left;
    let best = 0;
    for (let i = 1; i < xs.length; i++) if (Math.abs(xs[i] - x) < Math.abs(xs[best] - x)) best = i;
    setHover(best);
  }

  const h = hover != null && hover < history.length ? history[hover] : null;
  const latest = history.length ? history[history.length - 1] : null;
  const grid = [0, 0.5, 1];

  return (
    <div className="space-y-2" data-speed-graph>
      <div className="flex items-center gap-4 text-xs text-zinc-400" data-graph-legend>
        <span className="flex items-center gap-1.5">
          <span className="inline-block w-3 h-0.5 rounded" style={{ background: DOWN_COLOUR }} /> Download
          {latest && <span className="text-zinc-300 tabular-nums">{speed(latest[1], bits)}</span>}
        </span>
        <span className="flex items-center gap-1.5">
          <span className="inline-block w-3 h-0.5 rounded" style={{ background: UP_COLOUR }} /> Upload
          {latest && <span className="text-zinc-300 tabular-nums">{speed(latest[2], bits)}</span>}
        </span>
      </div>
      <div ref={boxRef} className="relative w-full">
        {history.length < 2 ? (
          <p className="text-xs text-zinc-500 py-12 text-center" data-graph-waiting>
            The graph fills in as GitSwitch measures, every update.
          </p>
        ) : (
          <svg
            width={width}
            height={HEIGHT}
            role="img"
            aria-label={`Download and upload speed over the last ${Math.round((t1 - t0) / 1000)} seconds`}
            tabIndex={0}
            className="block outline-none focus-visible:ring-1 focus-visible:ring-zinc-500 rounded"
            onPointerMove={(e) => pick(e.clientX, e.currentTarget)}
            onPointerLeave={() => setHover(null)}
            onKeyDown={(e) => {
              if (e.key === "ArrowLeft") setHover((v) => Math.max(0, (v ?? history.length - 1) - 1));
              else if (e.key === "ArrowRight") setHover((v) => Math.min(history.length - 1, (v ?? history.length - 1) + 1));
              else if (e.key === "Escape") setHover(null);
            }}
          >
            {grid.map((g) => {
              const y = PAD.top + plotH - g * plotH;
              return (
                <g key={g}>
                  <line x1={PAD.left} x2={PAD.left + plotW} y1={y} y2={y} stroke="#3f3f46" strokeWidth={1} />
                  <text x={PAD.left - 8} y={y + 3} textAnchor="end" fontSize={10} fill="#a1a1aa" className="tabular-nums">
                    {speed(top * g, bits)}
                  </text>
                </g>
              );
            })}
            <text x={PAD.left} y={HEIGHT - 6} fontSize={10} fill="#71717a">
              {ago(t0)}
            </text>
            <text x={PAD.left + plotW} y={HEIGHT - 6} fontSize={10} fill="#71717a" textAnchor="end">
              now
            </text>
            <path d={area(yDown)} fill={DOWN_COLOUR} fillOpacity={0.1} />
            <path d={area(yUp)} fill={UP_COLOUR} fillOpacity={0.1} />
            <path d={line(yDown)} fill="none" stroke={DOWN_COLOUR} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
            <path d={line(yUp)} fill="none" stroke={UP_COLOUR} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
            {hover != null && h && (
              <g data-graph-crosshair>
                <line x1={xs[hover]} x2={xs[hover]} y1={PAD.top} y2={base} stroke="#a1a1aa" strokeWidth={1} />
                <circle cx={xs[hover]} cy={yDown[hover]} r={4} fill={DOWN_COLOUR} stroke="#1f1f22" strokeWidth={2} />
                <circle cx={xs[hover]} cy={yUp[hover]} r={4} fill={UP_COLOUR} stroke="#1f1f22" strokeWidth={2} />
              </g>
            )}
          </svg>
        )}
        {hover != null && h && (
          <div
            className="pointer-events-none absolute top-1 rounded-md border border-zinc-700 bg-zinc-900/95 px-2 py-1 text-[11px] text-zinc-300 shadow"
            style={{ left: Math.min(Math.max(0, xs[hover] + 10), Math.max(0, width - 150)) }}
            data-graph-tooltip
          >
            <div className="text-zinc-500">{ago(h[0])}</div>
            <div className="flex items-center gap-1.5">
              <span className="inline-block w-2 h-2 rounded-full" style={{ background: DOWN_COLOUR }} /> Download <span className="tabular-nums ml-auto pl-2">{speed(h[1], bits)}</span>
            </div>
            <div className="flex items-center gap-1.5">
              <span className="inline-block w-2 h-2 rounded-full" style={{ background: UP_COLOUR }} /> Upload <span className="tabular-nums ml-auto pl-2">{speed(h[2], bits)}</span>
            </div>
          </div>
        )}
      </div>
      {history.length > 0 && (
        <details className="text-xs text-zinc-500">
          <summary className="cursor-pointer select-none">The numbers, as a table</summary>
          <table className="mt-1 w-full max-w-md text-left tabular-nums" data-graph-table>
            <thead className="text-zinc-400">
              <tr>
                <th className="font-normal py-0.5">When</th>
                <th className="font-normal">Download</th>
                <th className="font-normal">Upload</th>
              </tr>
            </thead>
            <tbody className="text-zinc-300">
              {[...history].reverse().slice(0, 30).map((r) => (
                <tr key={r[0]}>
                  <td className="py-0.5 text-zinc-500">{ago(r[0])}</td>
                  <td>{speed(r[1], bits)}</td>
                  <td>{speed(r[2], bits)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </details>
      )}
    </div>
  );
}
