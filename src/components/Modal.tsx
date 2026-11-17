import { ReactNode, useId } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

interface ModalProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  /** "lg" for content with rows (a branch list); "md" for a confirmation. */
  size?: "md" | "lg";
  /**
   * A click on the dimmed area around the dialog closes it (the default).
   * Off while that click would lose something: a name typed, an approval
   * being waited for. The ✕ button and the dialog's own buttons still close it.
   */
  closeOnBackdrop?: boolean;
}

/**
 * A dialog over the whole window. Drawn into <body>, not where it is used: a
 * dialog opened from the repository strip belongs to a tab that may be hidden
 * (display: none hides fixed children too), and it must show all the same.
 * Named for screen readers by its title; the page behind it is left out.
 */
export function Modal({ open, onClose, title, children, size = "md", closeOnBackdrop = true }: ModalProps) {
  const titleId = useId();
  if (!open) return null;

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div
        aria-hidden="true"
        className="absolute inset-0 bg-black/60 backdrop-blur-sm"
        onClick={closeOnBackdrop ? onClose : undefined}
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className={`relative w-full ${size === "lg" ? "max-w-2xl" : "max-w-md"} max-h-[calc(100vh-2rem)] overflow-y-auto rounded-xl border border-zinc-700/50 bg-zinc-900 p-6 shadow-2xl`}
      >
        <div className="flex items-center justify-between mb-4">
          <h2 id={titleId} className="text-lg font-semibold text-zinc-100">
            {title}
          </h2>
          <button
            type="button"
            aria-label="Close dialog"
            onClick={onClose}
            className="p-1 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer"
          >
            <X size={18} />
          </button>
        </div>
        {children}
      </div>
    </div>,
    document.body
  );
}
