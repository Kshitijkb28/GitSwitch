import { Check } from "lucide-react";

interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
}

/**
 * Dark-theme checkbox.
 *
 * A native `<input>` styled with `accent-color` still paints its UNCHECKED box
 * with the OS default (white on macOS), which looks broken on a dark surface.
 * So the real input is kept for semantics — label clicks, keyboard, focus,
 * screen readers — but visually hidden, and a styled box is drawn in its place.
 */
export function Checkbox({
  checked,
  onChange,
  disabled = false,
  className = "",
  ...rest
}: CheckboxProps) {
  return (
    <span className={`relative inline-flex shrink-0 ${className}`}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="peer absolute inset-0 w-full h-full opacity-0 m-0 cursor-pointer disabled:cursor-not-allowed"
        {...rest}
      />
      <span
        aria-hidden="true"
        className={`w-[18px] h-[18px] rounded-[5px] border flex items-center justify-center transition-colors
          peer-focus-visible:ring-2 peer-focus-visible:ring-emerald-500/50 peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-zinc-900
          ${
            checked
              ? "bg-emerald-500 border-emerald-500"
              : "bg-zinc-800 border-zinc-600 peer-hover:border-zinc-500"
          }
          ${disabled ? "opacity-50" : ""}`}
      >
        {checked && (
          <Check size={12} strokeWidth={3.5} className="text-zinc-950" />
        )}
      </span>
    </span>
  );
}
