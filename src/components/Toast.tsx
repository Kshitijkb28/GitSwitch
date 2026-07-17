import {
  createContext,
  useCallback,
  useContext,
  useState,
  ReactNode,
} from "react";
import { CheckCircle2, XCircle, Info, X } from "lucide-react";

type ToastVariant = "success" | "error" | "info";

interface Toast {
  id: number;
  message: string;
  variant: ToastVariant;
}

interface ToastContextValue {
  show: (message: string, variant?: ToastVariant) => void;
  success: (message: string) => void;
  error: (message: string) => void;
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

  const remove = useCallback((id: number) => {
    setToasts((t) => t.filter((x) => x.id !== id));
  }, []);

  const show = useCallback(
    (message: string, variant: ToastVariant = "info") => {
      const id = nextId++;
      setToasts((t) => [...t, { id, message, variant }]);
      setTimeout(() => remove(id), 3200);
    },
    [remove]
  );

  const value: ToastContextValue = {
    show,
    success: (m) => show(m, "success"),
    error: (m) => show(m, "error"),
  };

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="fixed bottom-4 right-4 z-[100] flex flex-col gap-2 pointer-events-none">
        {toasts.map((t) => (
          <ToastCard key={t.id} toast={t} onClose={() => remove(t.id)} />
        ))}
      </div>
    </ToastContext.Provider>
  );
}

function ToastCard({ toast, onClose }: { toast: Toast; onClose: () => void }) {
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

  return (
    <div
      className={`pointer-events-auto flex items-center gap-3 min-w-[260px] max-w-sm px-4 py-3 rounded-xl border ${s.border} bg-zinc-900/95 backdrop-blur shadow-2xl animate-[toastIn_0.18s_ease-out]`}
    >
      {s.icon}
      <span className="text-sm text-zinc-100 flex-1">{toast.message}</span>
      <button
        onClick={onClose}
        className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-500 hover:text-zinc-200 transition-colors cursor-pointer"
      >
        <X size={14} />
      </button>
    </div>
  );
}
