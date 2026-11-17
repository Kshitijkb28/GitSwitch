import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
  ReactNode,
} from "react";
import { CheckCircle2, XCircle, Info, X } from "lucide-react";

type ToastVariant = "success" | "error" | "info";

interface Toast {
  id: number;
  message: string;
  variant: ToastVariant;
  /** What happened, line by line: what a save changed, what an action did. */
  details?: string[];
}

interface ToastContextValue {
  show: (message: string, variant?: ToastVariant, details?: string[]) => void;
  success: (message: string, details?: string[]) => void;
  error: (message: string, details?: string[]) => void;
  /** Neither good nor bad news: a hand-off, a pause. */
  info: (message: string, details?: string[]) => void;
}

/** Lines shown before "and N more". */
const MAX_LINES = 8;

/**
 * How long a toast stays: long enough to read. A short message gets 3.2
 * seconds; every word past eight adds 0.3 seconds (about 200 words a
 * minute), and every line below it 1.5 seconds; at most 15 seconds. The
 * pointer over it pauses it.
 */
export function toastDuration(message: string, details?: string[]): number {
  const words = message.trim().split(/\s+/).filter(Boolean).length;
  const lines = Math.min(details?.length ?? 0, MAX_LINES + 1);
  return Math.min(3200 + Math.max(0, words - 8) * 300 + lines * 1500, 15000);
}

const ToastContext = createContext<ToastContextValue | null>(null);

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used within <ToastProvider>");
  return ctx;
}

let nextId = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  // Each toast's clock: paused while the pointer is over it, so a list being
  // read doesn't vanish under the mouse.
  const clocks = useRef(new Map<number, { handle: number; due: number }>());

  const remove = useCallback((id: number) => {
    const c = clocks.current.get(id);
    if (c) window.clearTimeout(c.handle);
    clocks.current.delete(id);
    setToasts((t) => t.filter((x) => x.id !== id));
  }, []);

  const start = useCallback(
    (id: number, ms: number) => {
      const handle = window.setTimeout(() => remove(id), ms);
      clocks.current.set(id, { handle, due: Date.now() + ms });
    },
    [remove]
  );

  const pause = useCallback((id: number) => {
    const c = clocks.current.get(id);
    if (!c || !c.handle) return;
    window.clearTimeout(c.handle);
    // Kept as the time it had left, in `due` (now relative), until resumed.
    clocks.current.set(id, { handle: 0, due: Math.max(0, c.due - Date.now()) });
  }, []);

  const resume = useCallback(
    (id: number) => {
      const c = clocks.current.get(id);
      if (!c || c.handle) return;
      start(id, Math.max(c.due, 1500));
    },
    [start]
  );

  const show = useCallback(
    (message: string, variant: ToastVariant = "info", details?: string[]) => {
      const id = nextId++;
      const lines = details?.filter((d) => d.trim() !== "");
      setToasts((t) => [...t, { id, message, variant, details: lines?.length ? lines : undefined }]);
      start(id, toastDuration(message, lines));
    },
    [start]
  );

  const value: ToastContextValue = {
    show,
    success: (m, d) => show(m, "success", d),
    error: (m, d) => show(m, "error", d),
    info: (m, d) => show(m, "info", d),
  };

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="fixed bottom-4 right-4 z-[100] flex flex-col gap-2 pointer-events-none">
        {toasts.map((t) => (
          <ToastCard key={t.id} toast={t} onClose={() => remove(t.id)} onPause={() => pause(t.id)} onResume={() => resume(t.id)} />
        ))}
      </div>
    </ToastContext.Provider>
  );
}

function ToastCard({ toast, onClose, onPause, onResume }: { toast: Toast; onClose: () => void; onPause: () => void; onResume: () => void }) {
  const styles: Record<ToastVariant, { border: string; icon: ReactNode }> = {
    success: {
      border: "border-emerald-500/40",
      icon: <CheckCircle2 size={18} className="text-emerald-400 shrink-0" />,
    },
    error: {
      border: "border-red-500/40",
      icon: <XCircle size={18} className="text-red-400 shrink-0" />,
    },
    info: {
      border: "border-zinc-600/50",
      icon: <Info size={18} className="text-zinc-300 shrink-0" />,
    },
  };
  const s = styles[toast.variant];
  const details = toast.details ?? [];

  return (
    <div
      role="status"
      data-toast={toast.variant}
      onMouseEnter={onPause}
      onMouseLeave={onResume}
      className={`pointer-events-auto flex ${details.length ? "items-start max-w-md" : "items-center max-w-sm"} gap-3 min-w-[260px] px-4 py-3 rounded-xl border ${s.border} bg-zinc-900/95 backdrop-blur shadow-2xl animate-[toastIn_0.18s_ease-out]`}
    >
      {s.icon}
      <div className="flex-1 min-w-0">
        <p className="text-sm text-zinc-100">{toast.message}</p>
        {details.length > 0 && (
          <ul className="mt-1.5 space-y-0.5 pl-4 list-disc text-xs text-zinc-300" data-toast-details>
            {details.slice(0, MAX_LINES).map((d, i) => (
              <li key={i} className="[overflow-wrap:anywhere]">
                {d}
              </li>
            ))}
            {details.length > MAX_LINES && <li className="list-none -ml-4 text-zinc-500">and {details.length - MAX_LINES} more</li>}
          </ul>
        )}
      </div>
      <button
        onClick={onClose}
        aria-label="Close"
        className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-500 hover:text-zinc-200 transition-colors cursor-pointer"
      >
        <X size={14} />
      </button>
    </div>
  );
}
