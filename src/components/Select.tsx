import { useEffect, useMemo, useRef, useState, ReactNode } from "react";
import { ChevronDown, Check, Search } from "lucide-react";

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
  /**
   * Force the filter box on or off. Left unset, it appears on its own once a
   * list is longer than `SEARCH_FROM`. Two or three entries are a glance, not
   * a search; beyond that, typing beats reading.
   */
  searchable?: boolean;
  /** Names the filter box for screen readers, and fills its placeholder. */
  searchLabel?: string;
  className?: string;
}

/** Longer than this and the list gets a filter box without being asked.
 *  Three or fewer is a glance; a fourth is already worth typing into. */
const SEARCH_FROM = 3;
/** Rows put in the DOM at once; more are added as the list is scrolled. */
const PAGE = 50;

export function Select({
  value,
  onChange,
  options,
  placeholder = "Select…",
  disabled = false,
  optionIcon,
  searchable,
  searchLabel = "Search",
  className = "",
}: SelectProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(0);
  const [visible, setVisible] = useState(PAGE);
  const canSearch = searchable ?? options.length > SEARCH_FROM;
  const ref = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const selected = options.find((o) => o.value === value);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!canSearch || !q) return options;
    // The value is matched too: a repository's label is its name, but its
    // value is the full path, which is what tells two same-named clones apart.
    return options.filter(
      (o) => o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q)
    );
  }, [options, query, canSearch]);

  // Callers build their option list inline, so it is a new array on every
  // render. Depending on it here would reset the query on each keystroke, so
  // the current list and choice are read through refs and this runs only when
  // the menu actually opens.
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const valueRef = useRef(value);
  valueRef.current = value;

  // A fresh filter every time it opens, and the current choice under the
  // cursor so Enter without typing changes nothing.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    const at = optionsRef.current.findIndex((o) => o.value === valueRef.current);
    setHighlight(at >= 0 ? at : 0);
    // Open on the current choice even when it sits far down a long list.
    setVisible(Math.max(PAGE, at + PAGE));
    if (canSearch) {
      // After the list has rendered, or there is nothing to focus yet.
      const id = window.setTimeout(() => searchRef.current?.focus(), 0);
      return () => window.clearTimeout(id);
    }
  }, [open, canSearch]);

  // Walking down with the keyboard has to reach past the rendered window.
  useEffect(() => {
    if (highlight >= visible) setVisible(Math.ceil((highlight + 1) / PAGE) * PAGE);
  }, [highlight, visible]);

  // Moving past the visible window should bring the highlighted row into view.
  useEffect(() => {
    if (!open) return;
    listRef.current?.querySelector('[data-highlighted="true"]')?.scrollIntoView({ block: "nearest" });
  }, [open, highlight]);

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

  const choose = (v: string) => {
    onChange(v);
    setOpen(false);
  };

  /** Arrows move, Enter picks, so the list is usable without the mouse. */
  const onListKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (shown.length === 0) return;
      setHighlight((i) => {
        const at = i < shown.length ? i : 0;
        const next = e.key === "ArrowDown" ? at + 1 : at - 1;
        return (next + shown.length) % shown.length;
      });
    } else if (e.key === "Enter") {
      e.preventDefault();
      const opt = shown[highlight];
      if (opt) choose(opt.value);
    }
  };

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
        <div className="absolute z-50 mt-1.5 w-full rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl overflow-hidden animate-[toastIn_0.12s_ease-out]">
          {canSearch && (
            <div className="relative border-b border-zinc-800 p-1.5">
              <Search size={14} className="absolute left-3.5 top-1/2 -translate-y-1/2 text-zinc-500" />
              <input
                ref={searchRef}
                type="text"
                aria-label={searchLabel}
                placeholder={searchLabel}
                value={query}
                onChange={(e) => {
                  setQuery(e.target.value);
                  setHighlight(0);
                  setVisible(PAGE);
                }}
                onKeyDown={onListKey}
                className="w-full rounded-md bg-zinc-800 border border-zinc-700 pl-7 pr-2 py-1.5 text-sm text-zinc-100 placeholder-zinc-500 focus:outline-none focus:border-emerald-500 focus:ring-2 focus:ring-emerald-500/40"
              />
            </div>
          )}
          <div
            ref={listRef}
            onScroll={(e) => {
              // Near the bottom, put the next page in the DOM. Thousands of
              // rows at once is what makes a long list feel stuck.
              const el = e.currentTarget;
              if (el.scrollHeight - el.scrollTop - el.clientHeight < 64) {
                setVisible((n) => (n < shown.length ? n + PAGE : n));
              }
            }}
            className="max-h-64 overflow-y-auto py-1"
          >
            {shown.length === 0 ? (
              <div className="px-3 py-2 text-sm text-zinc-500">
                {options.length === 0
                  ? "No options"
                  : `Nothing matches “${query.trim()}”`}
              </div>
            ) : (
              shown.slice(0, visible).map((opt, i) => {
                const active = opt.value === value;
                const cursor = canSearch && i === highlight;
                return (
                  <button
                    key={opt.value}
                    type="button"
                    data-highlighted={cursor ? "true" : undefined}
                    onMouseEnter={() => canSearch && setHighlight(i)}
                    onClick={() => choose(opt.value)}
                    className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-left transition-colors cursor-pointer ${
                      active
                        ? "bg-emerald-500/15 text-emerald-300"
                        : cursor
                          ? "bg-zinc-800 text-zinc-100"
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
            {shown.length > visible && (
              <p className="px-3 py-2 text-xs text-zinc-500">
                Showing {visible} of {shown.length}. Keep scrolling{canSearch ? ", or type to narrow it down" : ""}.
              </p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
