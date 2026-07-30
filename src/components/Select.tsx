import { useEffect, useRef, useState, ReactNode } from "react";
import { ChevronDown, Check } from "lucide-react";

export interface SelectOption {
  value: string;
  label: string;
}

interface SelectProps {
  value: string;
  onChange: (value: string) => void;
  options: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  /** Optional element rendered before each option's label (e.g. an icon). */
  optionIcon?: ReactNode;
  className?: string;
}

export function Select({
  value,
  onChange,
  options,
  placeholder = "Select…",
  disabled = false,
  optionIcon,
  className = "",
}: SelectProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const selected = options.find((o) => o.value === value);

  useEffect(() => {
    if (!open) return;
    function onDocClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className={`relative ${className}`}>
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        className={`w-full flex items-center justify-between gap-2 px-3 py-2 rounded-lg text-sm text-left transition-colors bg-zinc-800 border ${
          open
            ? "border-emerald-500 ring-2 ring-emerald-500/40"
            : "border-zinc-700 hover:border-zinc-600"
        } text-zinc-100 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer`}
      >
        <span className={`flex items-center gap-2 truncate ${selected ? "text-zinc-100" : "text-zinc-500"}`}>
          {selected && optionIcon}
          <span className="truncate">{selected ? selected.label : placeholder}</span>
        </span>
        <ChevronDown
          size={16}
          className={`shrink-0 text-zinc-400 transition-transform ${open ? "rotate-180" : ""}`}
        />
      </button>

      {open && (
        <div className="absolute z-50 mt-1.5 w-full rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl overflow-hidden animate-[toastIn_0.12s_ease-out] max-h-64 overflow-y-auto py-1">
          {options.length === 0 ? (
            <div className="px-3 py-2 text-sm text-zinc-500">No options</div>
          ) : (
            options.map((opt) => {
              const active = opt.value === value;
              return (
                <button
                  key={opt.value}
                  type="button"
                  onClick={() => {
                    onChange(opt.value);
                    setOpen(false);
                  }}
                  className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-left transition-colors cursor-pointer ${
                    active
                      ? "bg-emerald-500/15 text-emerald-300"
                      : "text-zinc-200 hover:bg-zinc-800"
                  }`}
                >
                  {optionIcon && (
                    <span className={active ? "text-emerald-400" : "text-zinc-400"}>
                      {optionIcon}
                    </span>
                  )}
                  <span className="truncate flex-1">{opt.label}</span>
                  {active && <Check size={15} className="shrink-0 text-emerald-400" />}
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}
