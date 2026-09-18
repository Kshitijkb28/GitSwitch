import { TextareaHTMLAttributes } from "react";

interface TextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  label?: string;
  error?: string;
  hint?: string;
  containerClassName?: string;
}

export function Textarea({
  label,
  error,
  hint,
  className = "",
  containerClassName = "",
  ...props
}: TextareaProps) {
  return (
    <div className={containerClassName}>
      {label && (
        <label className="block text-sm font-medium text-zinc-300 mb-1.5">
          {label}
        </label>
      )}
      <textarea
        className={`w-full px-3 py-2 rounded-lg bg-zinc-800 border text-sm text-zinc-100 placeholder-zinc-500 focus:outline-none focus:ring-2 focus:ring-emerald-500/50 focus:border-emerald-500 resize-y ${
          error ? "border-red-500/50" : "border-zinc-700"
        } ${className}`}
        {...props}
      />
      {error ? (
        <p className="mt-1 text-xs text-red-400">{error}</p>
      ) : hint ? (
        <p className="mt-1 text-xs text-zinc-500">{hint}</p>
      ) : null}
    </div>
  );
}
