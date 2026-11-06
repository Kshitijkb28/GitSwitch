import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import * as api from "../../lib/api";
import { DOWN_COLOUR, UP_COLOUR } from "./SpeedGraph";

/**
 * The speed bar's own window: the speed in two short lines, over the Windows
 * taskbar (clear background) or as a small strip elsewhere. Drag it anywhere;
 * double-click opens the Net speed tab.
 */
export function SpeedBar({ transparent }: { transparent: boolean }) {
  const [bar, setBar] = useState<api.NetBar | null>(null);
  // Dragged by hand once the pointer moves: a click or double-click stays a
  // click (a drag region's double-click would maximize the bar).
  const pressed = useRef<{ x: number; y: number } | null>(null);

  useEffect(() => {
    if (transparent) {
      document.documentElement.style.background = "transparent";
      document.body.style.background = "transparent";
    }
    let alive = true;
    const read = () => api.netspeedBar().then((b) => alive && setBar(b), () => {});
    read();
    const t = window.setInterval(read, 1000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [transparent]);

  const ink = bar?.light ? "#18181b" : "#f4f4f5";
  const lines: [string, string, string][] = [];
  if (bar && bar.show !== "down") lines.push(["↑", bar.up, UP_COLOUR]);
  if (bar && bar.show !== "up") lines.push(["↓", bar.down, DOWN_COLOUR]);

  return (
    <div
      data-speed-bar
      title="GitSwitch net speed — drag to move, double-click to open"
      onMouseDown={(e) => {
        if (e.button === 0) pressed.current = { x: e.screenX, y: e.screenY };
      }}
      onMouseMove={(e) => {
        const p = pressed.current;
        if (p && e.buttons & 1 && Math.hypot(e.screenX - p.x, e.screenY - p.y) > 3) {
          pressed.current = null;
          getCurrentWindow().startDragging().catch(() => {});
        }
      }}
      onMouseUp={() => {
        pressed.current = null;
      }}
      onDoubleClick={() => api.netspeedShowTab().catch(() => {})}
      className={`h-screen w-screen select-none flex flex-col justify-center px-2 cursor-default ${transparent ? "" : "bg-zinc-900 rounded-md"}`}
      style={{ color: ink, fontSize: 11, lineHeight: "14px" }}
    >
      {lines.map(([arrow, text, colour]) => (
        <div key={arrow} className="flex items-center justify-end gap-1.5 tabular-nums font-medium whitespace-nowrap">
          <span style={{ color: colour }}>{arrow}</span>
          <span>{text}</span>
        </div>
      ))}
    </div>
  );
}
